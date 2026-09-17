# codex-acp outgoing adapter (T013)

Cortex is an outgoing ACP v1 client of a pinned, operator-verified
`@agentclientprotocol/codex-acp` install (Node.js entry point over the Codex
App Server) — the exact same outgoing-client role documented for
`engine.session_manager.EngineSessionManager` (see T012). This page covers
the codex-acp-specific pieces added in T013:
`plugins/cortex/scripts/cortex_runtime/engine/codex_install.py` (pinned
install identity/integrity) and
`plugins/cortex/scripts/cortex_runtime/engine/codex_adapter_acp.py`
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

## Pinned install requirements (manual recipe placeholder)

This task implements **no installer** (a real download/install pipeline is
explicitly out of scope here — see T031). An operator who has already
installed codex-acp by some other means (for example following Zed's own
npx-based Custom Agent registration, or a manual `npm install
@agentclientprotocol/codex-acp`) records the resulting install as a
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
- any path under `$HOME/.local/share/zed/external_agents` (a
  client-managed registry checkout, refreshed/replaced by Zed itself,
  never something this project pins) or `$HOME/.local/share/app.codeg` /
  `$HOME/.config/app.codeg` (codeg's own private data directory — codeg is
  no longer a client under active qualification, but these locations
  remain rejected as harmless, safer defense-in-depth if codeg is still
  present on a given machine) — `engine_install_untrusted_location`.
  Whether AionUI (the current second real client, see
  [aionui-custom-agent.md](aionui-custom-agent.md)) downloads/manages ACP
  CLI adapters into any concrete, similarly-untrusted location under
  `$HOME/.config/AionUi` or elsewhere is **UNKNOWN** (not verified from
  `/opt/AionUi`'s bundled app files, and this task does not read anything
  under the live `~/.config/AionUi` user data directory); no such rejection
  is added here until it is confirmed;
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

## Instruction delivery (unqualified)

`engine.codex_adapter_acp.build_first_prompt_blocks(instructions_text,
user_blocks)` prepends one leading `{"type": "text", "text":
instructions_text}` block ahead of the caller's own prompt content blocks.
This is the demonstrated instruction-delivery mechanism for the pinned
release, and it is **documented as unqualified**: nothing in the pinned
release's static surface confirms the engine treats a leading block any
differently from an ordinary user message (no distinct "system"/
"instructions" role is exposed on the wire). It never writes an
`AGENTS.md` file into the workspace and never sets a `CODEX_CONFIG`
environment variable — those are different, out-of-scope delivery
mechanisms this task does not implement.

## MCP bridge (unqualified)

The single Cortex MCP server entry Cortex passes on `session/new` (see
`engine.mcp_bridge.build_mcp_servers`, from T012) is passed to codex-acp
identically to every other engine; its schema shape matches the pinned
release's observed `mcpServers` handling (`dist/index.js` ~lines
19535–19552), but whether the pinned Codex App Server actually spawns and
uses it correctly is not independently re-verified here beyond what T012's
own contract tests already cover generically. Treat MCP tool delivery
through this pinned release as unqualified until a live probe (T035)
confirms it end to end.

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
module, deliberately named differently to avoid confusion), `server.py`,
`hooks.py`, `host_source.py`, `marketplace_bootstrap.py`, or anything under
a `.codex-plugin` path (see `tests/test_engine_codex_no_native_fallback.py`,
an AST-level import/string-literal boundary check that scans every
`cortex_runtime/engine/*.py` module, not only this task's own two). A
`ConfigurationRejected` or engine `PortableError` raised anywhere in this
adapter never triggers the legacy native path, silently or otherwise:
there is no code path from this module back into it. Comparing behavior
against the legacy native route remains a manual-only exercise (run both
side by side and diff by hand) — this task adds no automated
legacy-comparison/rollback switch.

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
