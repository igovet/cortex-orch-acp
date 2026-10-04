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

Worker role instructions are maintained in canonical `skills/worker-*/SKILL.md`
sources and composed with shared protocol text. The package no longer includes
redundant `agents/*.toml` worker profiles or generated copies under
`agents/references/`; `profiles.json` maps role entries to Markdown sources, and
`python3 -B scripts/generate_agent_profiles.py check` validates generated
instruction artifacts. The 23 role instructions remain; the duplicate TOML
serialization format does not.

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
the ACP cleanup. Client registration and setup are verified through
[zed-custom-agent.md](zed-custom-agent.md) and
[codeg-custom-agent.md](codeg-custom-agent.md); any retained evidence lives
under `docs/acp/evidence/`. No retired native CLI/Desktop harness is a
current reference.

### Launch-time environment variables

| Variable | Used by | Status |
|---|---|---|
| `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | `cortex_runtime.connections.registry` and everything built on it (connections, catalog, user defaults) | ACP path, stays |
| `CORTEX_OBSERVATION_DIR` | `cortex_runtime.observability`/diagnostics logging sink | ACP path, stays |
| `CODEX_HOME` | Native codex-acp/codex CLI isolation directory | ACP path (isolation) stays; the native-profile-registration use (`cortex_setup.py`) was Codex-harness-era and was removed at T033 |

## Pinned dependencies

`./pinned-dependencies.json` is the single manifest for:

- the Python version floor (currently 3.11+, matching
  the Python version declared by the current runtime environment);
- this package's own portable contract version
  (`cortex_runtime.portable.version.CONTRACT_VERSION`);
- the vendored ACP schema fixture's pinned sha256
  (`tests/fixtures/acp/schema-1.4.0.json`);
- each supported engine adapter's pinned compatibility basis (codex-acp
  2.0.1 with Codex CLI 0.159.1; Claude's instruction-channel basis remains
  0.78.0 while the installed current adapter is 0.84.0 — see
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
- **`add-opencode-connection [--binary PATH]`** onboards the optional
  OpenCode v2 engine, which is never discovered at startup. It finds a
  trusted `opencode` binary and reads its version in a network-less sandbox,
  refusing versions outside `2.x.x`. It then registers a connection that
  references the owner's own `~/.local/share/opencode` login (never copied)
  and prints the next step: `opencode auth login`, `chmod 700` of that
  directory, or `refresh-catalog`. See the
  [OpenCode adapter](opencode-adapter.md#onboarding).
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

## Retention maintenance

Use `/cortex-clear` in an idle ACP session to preview old task deletion from
this project. It is advertised in the ACP command picker and handled by the
runtime without a coordinator turn or engine/model call; `/clear` is not an
alias because hosts reserve it.

```
/cortex-clear before 2026-10-01
/cortex-clear before 2026-10-01 confirm
```

The date means strictly earlier than 00:00 UTC; a bare `YYYY-MM-DD` also works.
For retention relative to now, use `7`, `7d` or `days 7` (integer 0..36500).
Invalid or future dates and malformed arguments return usage with no deletion.
Without trailing `confirm`, output gives `would_delete_tasks`,
`would_delete_orphan_sessions`, `skipped_protected`, the UTC cutoff and the
exact confirm command. With `confirm`, output gives `deleted_tasks`,
`deleted_orphan_sessions`, `skipped_protected` and the cutoff. Relative days
are recalculated when confirming; use an absolute date for a fixed boundary.
Neither output exposes task text or private reports.

Chat cleanup normally protects loaded-session tasks, every non-terminal runtime
task, unreleased capacity reservations and unterminated process records.
Unknown states and ambiguous activity remain protected. To delete stale
protected tasks, explicitly request force (still preview first):

```
/cortex-clear days 1 force
/cortex-clear days 1 force confirm
```

The same suffix works with `before YYYY-MM-DD`, bare dates, `N` and `Nd`.
Force always keeps the invoking session, its task and kept threads. It releases
stale reservations and marks process records terminated with an owner-force
assumption before replaying deletion. **It assumes those processes are already
dead and does not kill OS processes.** Do not use force on genuinely running work.
Deleted sessions are evicted from the agent's cache.

Old unbound sessions, or sessions referencing absent tasks, are also cleaned up:
their last activity is the maximum of session creation, close and latest
`host_transcript`/`host_updates` timestamps. The invoking session and, without
force, other loaded sessions are excluded. Transcript/update rows and session
workspace bindings are deleted before session metadata.

A nonblocking exclusive store lock fences concurrent access through selection,
committed deletion intents, filesystem cleanup and space reclamation. Busy access
or a pending intent conflicting with remaining keeps is refused even with force.
An in-flight prompt cannot consume this command as a question answer or queue it
as model work.

After a confirmed cleanup removes anything, SQLite `VACUUM` runs outside a
transaction while the exclusive lock remains held. Output reports vacuum status,
`reclaimed_bytes` and `db_bytes_before`/`db_bytes_after` when measured.
VACUUM may need substantial temporary disk space. A reclamation failure is
reported separately: **deletion remains committed**, not rolled back.
The manual `src/cortex_clear.py` CLI emits JSON with `deleted_tasks`,
`retention_days`, `skipped_protected`, `cutoff`, `force`,
`deleted_orphan_sessions`, `vacuum_status` and `reclaimed_bytes`; when a
VACUUM runs it adds `db_bytes_before` and `db_bytes_after`, or `vacuum_error`
on failure. `--dry-run` adds `would_delete_tasks` and
`would_delete_orphan_sessions`.

The CLI is still owner-run, never coordinator/worker-run:

```
python3 -B src/cortex_clear.py --project-root <path> --days N [--keep-thread <id>] [--force] [--dry-run]
```

Its canonical `--project-root`, `--days` (0..36500), and repeatable
`--keep-thread` flags retain their manual retention semantics. Unlike chat,
it deletes directly unless `--dry-run` is given and only protects explicitly
named threads: it already ignores active protection without `--force`.
The optional force flag explicitly marks stale live-state records
released/terminated before deletion, but never overrides kept threads.
Run it only when the project runtime is idle. Activity selection remains the
maximum of task creation and published report/pipeline edition update times.
Cleanup removes project task directories and unambiguously associated drafts;
transactional `pending_deletions` intents remain recoverable after interruption.
Pending committed deletions are included in preview/deletion counts without
being replayed during a dry run. CLI JSON is counts/settings and reclamation
metadata only; it includes the same orphan cleanup and vacuum behavior.

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

The plugin registers a Cortex Settings screen for supported owner/project configuration. Its values are stored by the Cortex runtime (JSON files or the existing project SQLite configuration row), not in Paseo's settings document. A live Paseo 0.10.0 check did not find the screen in the rendered Settings navigation or command palette, so do not assume it is reachable in that host version. See [configuration management](../configuration.md#manage-settings-in-paseo) for scope, revision conflicts, redaction and the OpenCode opt-in.

#### Install from GitHub

The plugin lives below the repository root, so include the Paseo subdirectory
suffix. Paseo stages the full Git checkout and runs the manifest's supported
`build` command before activation. The manifest calls
`scripts/cortex_acp_setup.py bootstrap-paseo-git-install`, which prepares the
private runtime and launch record. No separate `init-storage` or
`configure-paseo-plugin-launch` command is needed for this Git install; the
hook does not log in to an engine or copy credentials.

Install the current default ref:

```bash
paseo plugin install github:igovet/cortex-orch-acp:integrations/paseo
```

Select a branch, tag, or commit with `--ref`:

```bash
paseo plugin install github:igovet/cortex-orch-acp:integrations/paseo --ref dev
```

Paseo stages the complete repository, including `src/cortex_acp.py`; the
`:integrations/paseo` suffix selects the plugin entry directory. The build
step retains the launch-critical runtime under
`$XDG_STATE_HOME/cortex/paseo-runtime/` and writes a `0600` launch record at
`~/.config/cortex/paseo-plugin-launch.json`.

Update the installed Git plugin, checking first if desired:

```bash
paseo plugin update cortex-acp --check
paseo plugin update cortex-acp --ref dev --yes
```

For an isolated development daemon, use the guarded wrapper and a full absolute
checkout path. For a local directory install, install that plugin directory and
reload it after editing; directory sources are reloaded, not updated with
`plugin update`:

```bash
paseo plugin install /absolute/path/to/cortex-orch-acp/integrations/paseo
paseo plugin reload cortex-acp
```

An update or reload restarts the plugin process, which closes every open
Cortex chat's connection. Paseo then shows `Provider connection closed` on
each open chat, including completed ones. Cortex keeps the saved task outcome.
When a chat is reopened, it adds a notice with the saved state. Interrupted
workers are reconciled on the next message, and actions with an unknown
outcome are not re-run (see
[failures and disconnects](failures-and-cancellation.md#4-disconnect)). Until
that path is qualified in a live dev Paseo run, update between active tasks.

The managed local-checkout setup remains available while Paseo is running:

```bash
python3 -B scripts/cortex_acp_setup.py install-paseo-plugin
```

It validates and installs/reloads the repository-owned provider, then verifies
that it is running. This is the supported explicit local setup workflow.

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
the client setup guide for client-specific registration details.

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

## Model-policy installation and explicit migration

The public policy/model-settings-default.json is an installation-only, schema-validated candidate. It is packaged with the runtime, but runtime routing never reads or merges it. Fresh setup creates only absent settings files with private no-replacement writes; existing, partial, legacy owner settings and saved profiles remain unchanged. The candidate is derived from retained production configuration and observed route policy, not dev/example files. Its portable coordinator preference contains engine/model/parameters, not machine-specific route, connection or account identifiers. Installation is not evidence that production was updated or a live route qualified.

For an existing installation, derive a candidate from owner settings preserving explicit denies, providers, preferences and dormant policy. Run preview, review the diff/diagnostics and source/candidate digests, then apply only with explicit consent and the exact preview revision/digest. The tool verifies a private backup before replacement, verifies new bytes and attempts exact rollback on failure; rollback failure is explicit. Migrate each saved profile separately from that profile's own candidate. Do not apply a working-settings candidate to inactive profiles. Portable coordinator binding is accepted only for one exact currently selectable catalog identity and requested parameter set; unavailable or multiple matches require owner resolution, never automatic substitution.

Checkout commands:

    python3 -B scripts/cortex_model_policy_migrate.py preview --candidate /path/to/owner-candidate.json
    python3 -B scripts/cortex_model_policy_migrate.py apply --candidate /path/to/owner-candidate.json --expected-revision sha256:<preview-source-digest> --candidate-sha256 <preview-candidate-digest> --i-consent

Apply is a configuration mutation and requires explicit owner authorization. Saved-profile deletion is separate and revision-checked: confirmation identifies the exact profile; cancel changes nothing; deleting the selected profile clears its linkage without resetting materialized settings; deleting the last profile is permitted. Errors preserve profile/settings and require explicit reload before retry.

### Routing-only assertion preview

For a bounded, nonmutating check of current owner routing/default files:

    python3 -B scripts/cortex_routing_preview_check.py --config-root /absolute/config/cortex

This utility checks owner-private regular non-symlink files, a bounded allowlisted non-secret routing schema and coordinator/parameter shape; completes only missing fields from reviewed public installation data; asserts preservation and agreement between the migration API and real CLI preview; and verifies exact before/after bytes. It reads only routing/default files, not profiles, registry, accounts or logs. Unknown top-level routing keys refuse this narrow utility rather than being copied into a candidate; the general migration tool retains unknown owner fields and requires a purpose-specific owner candidate. Existing tier/list ordering is retained. The temporary candidate is private and deleted on exit; the output contains full hashes and assertion outcomes, not private identifiers or settings bodies. No apply is performed.

The apply command above holds the owner-private `connections.lock`, creates and verifies a new 0600 backup under `.migration-backups/`, and atomically replaces routing with 0600 bytes. Defaults and saved profiles are outside a routing-only apply. Source or candidate drift requires a new preview. Automatic failure rollback verifies the original bytes; there is no public CLI action for restoring a backup after a successful migration. Such a later restore needs a separately authorized, lock-protected operation with current-source and backup-digest preconditions; do not use migration apply to bypass preservation checks. A preview does not qualify production adoption, live quota labels or an installed runtime/plugin.
