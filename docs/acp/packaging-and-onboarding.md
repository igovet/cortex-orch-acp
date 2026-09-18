# Packaging, onboarding, and diagnostics (T031)

This page covers how a local checkout becomes a runnable Cortex ACP agent:
install, pinned versions, first-run setup, diagnostics, uninstall, and
client registration. See [zed-custom-agent.md](zed-custom-agent.md) and
[codeg-custom-agent.md](codeg-custom-agent.md) for the client-side recipe
details this page's setup step delegates to (T011).

## Install

There is no separate "install" step: `plugins/cortex/scripts/cortex_acp.py`
runs directly, as plain `python3`, from this checkout:

```
python3 -B /absolute/path/to/plugins/cortex/scripts/cortex_acp.py
```

`cortex_runtime` (everything under `plugins/cortex/scripts/cortex_runtime/`)
is standard-library only — nothing to `pip install` for the agent itself.
`cortex_acp.py` never spawns a plugin host, never downloads or upgrades
anything at launch, and writes only JSON-RPC lines to stdout (every log
goes to stderr) — see `tests/test_acp_entrypoint_packaging.py` and
`tests/test_acp_stdout_purity.py`.

### What was Codex-plugin-harness-era and was removed (T033)

The Codex plugin/gateway harness was a separate, older way to run Cortex —
not the ACP path above — and was deleted entirely at T033: the owned
MITM/gateway supervisor (`cortex_gateway.py`, `cortex_runtime/gateway/`,
`cortex_runtime/runtime/`), the pinned-dependency pip installer
(`marketplace_bootstrap.py`), native Codex profile registration into
`~/.codex/agents` (`cortex_setup.py`), the local lifecycle hook dispatcher
(`plugins/cortex/hooks/`, `cortex_hook.py`, `cortex_runtime/hooks.py`), the
Codex-native unchanneled MCP `_meta` identity adapter (`codex_adapter.py`),
the native-index host-source reader (`host_source.py`), the
`.codex-plugin/` Marketplace manifest (the package identity/stamp now
lives in `plugins/cortex/package-identity.json` instead), and the
`phase2_cli_*`/`phase2` external-evaluator preparation scripts and tests.
This page never built on any of them, and `cortex_acp.py` never did
either — see `tests/test_engine_codex_no_native_fallback.py` for the
still-enforced boundary that no `cortex_runtime/engine/*.py` module may
import any of these names. Real Codex CLI/Desktop live-product claims for
the remaining native surface stay pending T035–T037.

Also removed (repository-root `scripts/`, not shipped in the packaged
plugin): `cortex-dev` (the isolated-candidate CLI-before-Desktop preparer;
already non-functional — it imported the deleted `cortex_runtime.runtime.state`
and ran a `prepare_codex.py` that no longer exists), `cortex-live-capture-sink`
(its only caller), and `cortex_dev_config.py` (its `mcp_servers`/isolated-profile
opt-in was reachable only through `cortex-dev`). `cortex-desktop-dev` and
`cortex-live-smoke` are kept, unrun: their `start`/`send`/`stop` launch paths
are equally non-functional against the removed harness, but their pure
policy/audit-classification functions (`worker_model_policy`,
`call_policy_violations`, `current_host_audit_exit_code`, decision-coordination
checks, including the `phase2-cli-*` receipt schema names referenced from
SECURITY.md and `tests/test_package.py`) remain the only existing reference
implementation and regression-tested specification this project holds for
those rules, exercised by `tests/test_model_policy.py`,
`tests/test_decision_coordination.py`, `tests/test_observer_evaluation_v2.py`,
`tests/test_senior_consultant.py`, `tests/test_native_worker_result_fallback.py`,
`tests/test_diagnostic_boundary.py` and `tests/test_advisory_lane.py`. Neither
file is a current procedure; see AGENTS.md's "Historical: Codex-plugin/
native-subagent CLI-before-Desktop qualification procedure" for the retired
instructions that used to name them.

### Launch-time environment variables

| Variable | Used by | Status |
|---|---|---|
| `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | `cortex_runtime.connections.registry` and everything built on it (connections, catalog, user defaults) | ACP path, stays |
| `CORTEX_OBSERVATION_DIR` | `cortex_runtime.observability`/diagnostics logging sink | ACP path, stays |
| `CODEX_HOME` | Native codex-acp/codex CLI isolation directory | ACP path (isolation) stays; the native-profile-registration use (`cortex_setup.py`) was Codex-harness-era and was removed at T033 |
| `CORTEX_DEPENDENCY_DIR` | Was `marketplace_bootstrap.py`'s pinned pip-install target | Codex-harness-era (gateway only); removed at T033, including from `.mcp.json`'s `env_vars` |

## Pinned dependencies

`plugins/cortex/pinned-dependencies.json` is the single manifest for:

- the Python version floor (currently 3.11+, matching
  `docs/project/conventions.md`);
- this package's own portable contract version
  (`cortex_runtime.portable.version.CONTRACT_VERSION`);
- the vendored ACP schema fixture's pinned sha256
  (`tests/fixtures/acp/schema-1.4.0.json`);
- each supported engine adapter's pinned package version (codex-acp
  1.12.0, claude-agent-acp 0.78.0 — see
  [codex-acp-adapter.md](codex-acp-adapter.md) and
  [claude-agent-acp-adapter.md](claude-agent-acp-adapter.md) for the
  verified-digest install-record mechanism those pinned versions rely on).

`cortex_runtime.packaging.pinned.verify_all()`/`verify_launch_critical()`
re-check every one of those *fresh* on every call and report a
`Finding(status="ok"|"mismatch"|"missing", repair_action=...)` — never a
cached verdict, never an auto-upgrade. `cortex_acp.py` runs
`verify_launch_critical()` (Python floor + contract version — the two
checks cheap and safe enough for every ordinary launch) before it ever
constructs the production runtime, and exits 1 with a concrete stderr
message on any mismatch. `scripts/cortex_diagnose.py` runs the full
`verify_all()` set, including the schema fixture digest and the engine pin
listing.

## First-run setup

`scripts/cortex_acp_setup.py` has four independent, idempotent,
non-destructive steps (`status`, the default, only reads):

```
python3 -B scripts/cortex_acp_setup.py status
python3 -B scripts/cortex_acp_setup.py init-storage
python3 -B scripts/cortex_acp_setup.py add-connection \
    --name work --engine codex-acp --provider openai \
    --auth-mode api_key --key-kind openai_api_key \
    --api-key-file /path/to/key.txt      # never inline on argv
python3 -B scripts/cortex_acp_setup.py pin-route \
    --connection cn_xxxxxxxxxxxx --model gpt-5-codex
python3 -B scripts/cortex_acp_setup.py record-observation \
    --connection cn_xxxxxxxxxxxx --model gpt-5-codex \
    --config-options-file /path/to/observed-config-options.json
python3 -B scripts/cortex_acp_setup.py print-client-recipes
```

- **`init-storage`** creates/verifies the owner-private
  `$XDG_CONFIG_HOME/cortex`/`$XDG_STATE_HOME/cortex` roots (`0700`,
  symlink-refusing — the same primitives
  `cortex_runtime.connections.registry` already uses). It never touches
  any project's own `.cortex` store.
- **`add-connection`** registers (or reuses) one connection through the
  existing `connections.onboarding` API. An API-key secret is always read
  from a file path or stdin — never a plain CLI argument — and is written
  straight to the connection's private secret file; nothing here ever
  logs or prints it. A native-account connection instead prints
  non-secret instructions (the isolated directory + env var name) for the
  user's own login flow, run entirely outside Cortex; Cortex never
  collects or stores that credential itself.
- **`pin-route`** declares one hand-picked, already-qualified
  `(engine, auth_mode, model)` combination (never a live engine probe —
  see `catalog.discovery.ManualDeclaration`; only a documented/qualified
  combination is ever accepted) and persists it as the same plain T008
  user-default route/parameters an in-session `/cortex-model` choice
  would write (`configuration.store.write_user_defaults`). A fresh task's
  first prompt then adopts this default automatically — see
  `acp.agent.Agent._propose_pending_selection_for_new_task` — with no
  further per-task action. `tests/test_acp_setup_fresh_session.py` proves
  this end to end (fake downstream engine subprocess, real production
  `Agent`/`OrchestratingRuntime` code).
- **`record-observation`** replaces a `pin-route`'d route with one built
  from a *real* engine-observed `configOptions` payload (the same
  `session/new`-shaped JSON array a live engine actually returned — a
  Zed devtools capture, or the result of an explicitly authorized
  `scripts/cortex_codex_probe.py` qualification run; this step itself
  never contacts an engine). Unlike `pin-route`'s `ManualDeclaration`
  (only ever accepted with **no** parameters, since nothing confirmed
  what it actually supports), this goes through
  `catalog.discovery.parse_observation`/`build_catalog` — the same
  DISCOVERY-provenance path automatic discovery would use — so the saved
  route's `ExecutionRoute.supported_parameters` are the engine's real
  advertised values (every value in each `mode`/`thought_level`
  select's own `options`, not only whichever was current when observed),
  and re-pins it as the user default exactly like `pin-route` does. This
  is what lets a real reasoning-effort choice (e.g. "high") actually
  appear as a selectable `thought_level` config option in a client's
  message panel — see `acp.config_options.build_thought_level_option`
  and `tests/test_cortex_acp_setup.py`'s `test_record_observation_*`
  tests.
- **`print-client-recipes`** delegates entirely to the existing T011
  `scripts/cortex_acp_client_config.py` (dry-run by default; a real write
  needs `--settings PATH --apply --i-consent-to-write`) plus the codeg
  recipe text — this script never re-implements or duplicates that write
  path.

## Diagnostics

`scripts/cortex_diagnose.py` checks storage layout/permissions, the
pinned-dependency manifest, connection/route availability, an optional
client-registration presence check (`--zed-settings PATH`, read-only),
adapter capability/qualification status, and (`--project PATH`) a
project's own policy-ceiling configuration — all offline, all read-only,
**never** a model call:

```
python3 -B scripts/cortex_diagnose.py                      # JSON, exit 0/1/2
python3 -B scripts/cortex_diagnose.py --format text
python3 -B scripts/cortex_diagnose.py --zed-settings ~/.config/zed/settings.json
```

Exit code distinguishes `ok` (0) / `degraded` (1, at least one repairable
gap) / `failed` (2, a hard/unsafe condition); every non-`ok` check names a
concrete `repair_action`.

`cortex_diagnose.py` never imports, execs, or subprocesses a paid-probe
script. The opt-in, explicitly-authorized live probes
(`scripts/cortex_codex_probe.py`, `scripts/cortex_claude_probe.py`) stay
entirely separate: they refuse to run at all without both
`--i-authorize-paid-call` and `--approval-ref`, and diagnostics has no
code path that reaches them — see `tests/test_cortex_diagnose.py`'s static
and dynamic (no-subprocess) guards.

## Uninstall

`scripts/cortex_uninstall.py` is dry-run by default (prints exactly what
would be removed, deletes nothing). Real deletion needs an explicit
`--remove-user-scope` and/or `--remove-project-store --project PATH`,
together with `--i-confirm-removal`:

```
python3 -B scripts/cortex_uninstall.py                                   # dry run
python3 -B scripts/cortex_uninstall.py --remove-user-scope --i-confirm-removal
python3 -B scripts/cortex_uninstall.py --project /path/to/project \
    --remove-project-store --i-confirm-removal
```

It deletes only `$XDG_CONFIG_HOME/cortex`, `$XDG_STATE_HOME/cortex`, and
(only when `--project` is given) that one project's own `.cortex` store —
each re-verified as an owner-private, non-symlinked real directory
immediately before deletion. It never touches engine installs, real
credentials outside those Cortex-owned roots, user history, project files
other than `.cortex`, or Git branches/working changes —
`tests/test_cortex_uninstall.py` plants unrelated files, a fake "engine
install" directory, and an uncommitted Git branch/working change
alongside a project's `.cortex` store and asserts all of them survive.

## Zed and codeg registration

See [zed-custom-agent.md](zed-custom-agent.md) and
[codeg-custom-agent.md](codeg-custom-agent.md) (T011) for the full
recipes; `scripts/cortex_acp_setup.py print-client-recipes` prints both
without writing anything unless explicitly told to.

## Prepared-but-not-published for the Registry

[registry-metadata-draft.md](registry-metadata-draft.md) (T011) is
explicitly **not submitted or published**; Custom Agent installation
(above) works with no Registry dependency at all — see
[real-client-runbook.md](real-client-runbook.md) for the still-NOT-RUN
live client rows owned by T035.
