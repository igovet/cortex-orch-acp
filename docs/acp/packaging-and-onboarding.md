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

### What is Codex-plugin-harness-era and is being removed (T033)

A few *other* files in this repository still belong to the older Codex
plugin/gateway harness, not the ACP path above, and are scheduled for
removal at T033 — this page never builds on them:

- `plugins/cortex/scripts/cortex_gateway.py`, `cortex_runtime/gateway/`,
  `cortex_runtime/runtime/` (the owned MITM/gateway supervisor).
- `cortex_runtime/marketplace_bootstrap.py` — pip-installs
  `requirements.lock` into `~/.codex/cortex/deps/<digest>` unless
  `CORTEX_DEPENDENCY_DIR` is already set. This is **only** reached by the
  gateway/MCP launch path, never by `cortex_acp.py`; it is the one place
  in this repository that does install third-party packages at (gateway)
  launch time, and it is exactly the behavior this task's entrypoint
  fail-closed check (below) was designed to *not* need or extend.
- `plugins/cortex/scripts/cortex_setup.py` — registers the 23 packaged
  agent profiles into Codex's own native `~/.codex/agents` registry
  (`CODEX_HOME`); this is Codex-CLI-native onboarding, unrelated to ACP
  client registration. `scripts/cortex_acp_setup.py` (this task) is a
  distinct, ACP-only script and does not call it.
- `plugins/cortex/hooks/`, `host_source.py`, `.codex-plugin/` marketplace
  wiring, legacy `.codex` paths, and the `phase2_cli_*`/`phase2` scripts
  and tests.

### Launch-time environment variables

| Variable | Used by | Status |
|---|---|---|
| `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | `cortex_runtime.connections.registry` and everything built on it (connections, catalog, user defaults) | ACP path, stays |
| `CORTEX_OBSERVATION_DIR` | `cortex_runtime.observability`/diagnostics logging sink | ACP path, stays |
| `CODEX_HOME` | Native codex-acp/codex CLI isolation directory *and* `cortex_setup.py`'s native-profile registration | ACP path (isolation) stays; the profile-registration use is Codex-harness-era, removed at T033 |
| `CORTEX_DEPENDENCY_DIR` | `marketplace_bootstrap.py`'s pinned pip-install target | Codex-harness-era (gateway only), removed at T033 |

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
