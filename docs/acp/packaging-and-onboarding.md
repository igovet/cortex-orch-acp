# Packaging, onboarding, and diagnostics (T031)

This page covers how a local checkout becomes a runnable Cortex ACP agent:
install, pinned versions, first-run setup, diagnostics, uninstall, and
client registration. See [zed-custom-agent.md](zed-custom-agent.md) and
[codeg-custom-agent.md](codeg-custom-agent.md) for the client-side recipe
details this page's setup step delegates to (T011).

## Install

There is no separate "install" step: `src/cortex_acp.py`
runs directly, as plain `python3`, from this checkout:

```
python3 -B /absolute/path/to/src/cortex_acp.py
```

`cortex_runtime` (everything under `src/cortex_runtime/`)
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
(`./hooks/`, `cortex_hook.py`, `cortex_runtime/hooks.py`), the
Codex-native unchanneled MCP `_meta` identity adapter (`codex_adapter.py`),
the native-index host-source reader (`host_source.py`), the
`.codex-plugin/` Marketplace manifest (the package identity/stamp now
lives in `./package-identity.json` instead), and the
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
opt-in was reachable only through `cortex-dev`). The old `cortex-desktop-dev`
and `cortex-live-smoke` launchers and their observer-only tests were removed in
the ACP cleanup. Real-client
qualification is defined by the ACP runbook and retains real client
observations; no retired native CLI/Desktop harness is a current reference.

### Launch-time environment variables

| Variable | Used by | Status |
|---|---|---|
| `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | `cortex_runtime.connections.registry` and everything built on it (connections, catalog, user defaults) | ACP path, stays |
| `CORTEX_OBSERVATION_DIR` | `cortex_runtime.observability`/diagnostics logging sink | ACP path, stays |
| `CODEX_HOME` | Native codex-acp/codex CLI isolation directory | ACP path (isolation) stays; the native-profile-registration use (`cortex_setup.py`) was Codex-harness-era and was removed at T033 |
| `CORTEX_DEPENDENCY_DIR` | Was `marketplace_bootstrap.py`'s pinned pip-install target | Codex-harness-era (gateway only); removed at T033 |

## Pinned dependencies

`./pinned-dependencies.json` is the single manifest for:

- the Python version floor (currently 3.11+, matching
  the Python version declared by the current runtime environment);
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
    --api-key-file /path/to/key.txt \
    --install-record /path/to/codex-install-record.json  # never inline secrets on argv
python3 -B scripts/cortex_acp_setup.py validate-connection \
    --connection cn_xxxxxxxxxxxx
python3 -B scripts/cortex_acp_setup.py pin-route \
    --connection cn_xxxxxxxxxxxx --model gpt-5-codex
python3 -B scripts/cortex_acp_setup.py record-observation \
    --connection cn_xxxxxxxxxxxx --model gpt-5-codex \
    --config-options-file /path/to/observed-config-options.json
python3 -B scripts/cortex_acp_setup.py print-client-recipes
python3 -B scripts/cortex_acp_setup.py install-paseo-plugin
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
  collects or stores that credential itself. `--install-record` is required
  and is verified for the selected engine before the registry is mutated;
  it must contain the exact JSON fields for that engine and never contains
  credentials.
- **`validate-connection`** recomputes one connection's current state and
  prints only its id, engine, state, and qualification flag. It never prints
  credential paths or values.
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

### Paseo provider plugin

Run one explicit setup command:

```bash
python3 -B scripts/cortex_acp_setup.py install-paseo-plugin
```

After changing the plugin, use the repository helper while Paseo is already
running:

```bash
scripts/update-paseo-plugin.sh
```

It restamps the package, installs/reloads the repository-owned provider, and
checks that it is running. It does not start or stop Paseo and does not touch
accounts, chats, or project-local `.cortex` stores. Set `CORTEX_PASEO_CLI` if
the Paseo executable is not on `PATH`.

`install-paseo-plugin` is the single explicit Cortex-on-Paseo installation
step. It validates the repository-owned `integrations/paseo` source and its
generated SHA-256 integrity record, enables the Paseo daemon plugin switch
only for this explicit setup action, installs or reloads the `cortex-acp`
provider, and verifies that the daemon reports it as `running`. It is
idempotent and preserves unrelated Paseo plugins and configuration keys.
Setup also writes the owner-private `HOME/.config/cortex/paseo-plugin-launch.json`
record with the absolute Cortex entrypoint and the non-secret `HOME`,
`XDG_CONFIG_HOME`, and `XDG_STATE_HOME` roots used by setup. The HOME-based
locator remains stable when a daemon restarts with a different XDG
environment. The provider applies those roots when it starts Cortex after a Paseo daemon restart, so the same connection
registry, catalog, and native-account state remain visible even when Paseo
was launched from a desktop environment with a different shell environment.
The record also names the repository-owned Paseo 0.8 `session/load` stdio
compatibility shim. Setup upgrades older argv-only records; a record without
the shim is rejected instead of silently launching an unpatched restart path.
The record contains no credentials and does not copy native account files.
Older argv-only records are upgraded by rerunning setup.
Its parent directories must be real, current-user-owned, owner-only
directories; setup refuses symlinked ancestors and uses exclusive private
temporary files for replacement. Setup also verifies that the daemon config is
current-user-owned and repairs group/world-readable mode bits before reading
it, without printing unrelated config keys.
The command never runs implicitly during an ACP session. After installation,
Paseo exposes the native Cortex provider; no separate generic ACP provider
registration is required for Paseo. The source hash is regenerated by
`python3 -B scripts/cortex_package.py stamp` and checked by package
validation. Paseo plugin code is trusted, unsandboxed daemon code, so this
explicit setup command is the required installation boundary.

The plugin manifest is named `cortex-acp`, but its registered Paseo provider
identity is `cortex` for new chats. The installer removes only the exact
repository-owned generic `cortex` entry after the plugin is running and saves
an owner-only configuration backup. Old generic chats may become inaccessible
after this migration; legacy chat compatibility is not guaranteed.

For catalog checks such as `paseo provider models cortex --json`, start or
restart the isolated Paseo daemon from the disposable project directory. Paseo
runs the provider's ACP `session/new` from the daemon's inherited cwd; if that
cwd is this Cortex checkout (whose `.cortex` store belongs to another project),
Cortex correctly fails closed with `project_storage_mismatch`. This is a
project-identity check, not a provider fallback; use the exact disposable
project intended for the qualification run.

## Prepared-but-not-published for the Registry

[registry-metadata-draft.md](registry-metadata-draft.md) (T011) is
explicitly **not submitted or published**; Custom Agent installation
(above) works with no Registry dependency at all — see
[real-client-runbook.md](real-client-runbook.md) for the still-NOT-RUN
live client rows owned by T035.

### Paseo update and existing MCP registries

`scripts/update-paseo-plugin.sh` calls `install-paseo-plugin`, which validates the
owner MCP registry before changing the daemon or launch record. A malformed or
pre-gateway entry without exact `allowed_tools` fails the update preflight with
its server name and required field. No missing grant is converted into wildcard
access. Repair enabled entries using the installed catalogue, explicit binary and
dependency `read_paths`, and only the required network permission.

An already-running `cortex-acp` plugin is now explicitly reloaded through Paseo's
installed `plugin reload` command. This restarts its provider process, so perform
updates between active tasks. A plugin's `running` state alone does not certify
MCP operations; retained startup and real gateway checks are recorded in the
[production repair observation](evidence/paseo-2026-09-21/production-mcp-repair.md).
