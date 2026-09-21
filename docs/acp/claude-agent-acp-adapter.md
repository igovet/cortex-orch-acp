# claude-agent-acp outgoing adapter (T026)

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
above was exercised live (see the T035/T036 rows at the end of this page).

Claude ACP reports Cortex MCP tool titles as `mcp__cortex__decide` (and can
omit the title on a later result update). The coordinator's pending-event
acceptance refusal boundary correlates those updates by tool-call reference,
so two exact refusals cancel only that engine turn and deliver the pending
event in the next continuation. A composed fake-ACP wire test covers this
transition; the 2026-09-20 live Sonnet run exposed the prior title mismatch,
but the corrected transition has not yet been observed in a real client.

For external MCP scalability, the pinned adapter forwards explicit
`session/new` servers into the Claude Agent SDK. The bundled SDK 0.3.270 types
describe deferred MCP tools when tool search is enabled, but the ACP adapter
does not report whether that search ran or expose a complete aggregate tool
count to Cortex. Cortex's registry filters whole servers by role and engine;
it does not filter individual tools or enforce a catalogue-size limit. Keep
each route's enabled server set narrow and use a server's own tool/service
filter for a large catalogue. A real session must retain tool-discovery and
call evidence before deferred loading is claimed to work for that route.

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
a bounded reason. An absent pre-prompt connection never counts as failure or
success. The relay token now has a fixed leading letter because an unprefixed
URL-safe token could begin with `-` and be parsed as an option by the
installed Python `argparse` relay launcher. This explains a reproducible
no-paid startup-close regression; the transient live failure's discarded
token bytes were not retained, so that individual case cannot be proved
byte-for-byte after the fact.

Every Cortex-created session at this pinned version also sends
`_meta.claudeCode.options.disallowedTools: ["Agent", "Task"]` and
`strictMcpConfig: true`, including a session without role text. The installed
adapter passes both into the Claude Agent SDK query before launch. The SDK
passes `--disallowedTools` and `--strict-mcp-config` to the CLI. The former's
declared contract removes those native subagent launch tools from the model
context and denies their execution. The latter's declared contract limits
MCP discovery to the session's explicit `mcpServers`, excluding Claude's
user/project settings, plugins, and on-disk agent frontmatter MCP servers.
This matters because the adapter otherwise loads user/project/local settings
and the discovered native-account connection uses the production
`CLAUDE_CONFIG_DIR` for authentication. This option does not disable non-MCP
plugin skills or hooks, and no live Claude call has yet verified the CLI's
behavior on a Cortex session.
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

**Still not live-executed** (T035/T036 mandatory gate, not satisfied by
any test here): no real turn has run through this channel. What a live
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

## T035/T036 live qualification rows: NOT RUN (blockers)

The following rows are **explicitly unrun live evidence**, never claimed
as passed contract fixtures (per `INVARIANTS.md` and this task's own "Live
verification boundary"):

- `outgoing-engine: claude-agent-acp` — every `status: "unqualified"` /
  `"unknown"` cell in `CAPABILITY_MATRIX.json` for this engine, pending a
  real, explicitly authorized run (T035).
- Live confirmation that the spawned Claude Code CLI actually *honors*
  `_meta.systemPrompt` as delivered — a real turn whose observable
  behavior demonstrably follows the injected instructions, not merely
  that the field reached the wire (which this task's tests do confirm
  statically/via the fake engine). `native_tool_containment` also stays
  `unknown` regardless: a system-prompt channel existing is not evidence
  of any sandbox around what it instructs.
- Real non-Codex coordinator execution (delegates a managed assignment,
  processes the result, participates in explicit `decide` acceptance) —
  **not run**; even a fixture/fake-engine pass is not release evidence
  (this task's own "Live verification boundary"). The opt-in, never-run
  script `scripts/cortex_claude_probe.py` documents the exact steps a
  future, explicitly authorized run would take (`--plan` mode only in this
  task).
- Exact-version drift re-check (0.78.0 vs whatever an operator's live
  install actually reports at run time) — not run; see `ADR-001b`'s own
  precedent for codex-acp's 1.11.0→1.12.0 drift.

These are assigned to **T035/T036**, never marked passed here.

See also [../index.md](index.md) and
[codex-acp-adapter.md](codex-acp-adapter.md) for the first engine's
equivalent page.
