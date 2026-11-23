# Packaging, onboarding, and diagnostics

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


This page covers how a local checkout becomes a runnable Cortex ACP agent:
install, pinned versions, first-run setup, diagnostics, uninstall, and
client registration. See the [Zed agent](../features/zed-agent/index.md) and
[codeg agent](../features/codeg-agent/index.md) pages for the client-side recipe
details this page's setup step delegates to.

## Install

There is no separate "install" step. The ACP entry is the Rust `cortex-acp` binary built from this checkout (see [default launch](native-incoming-acp.md#default-launch)); the product contains no Python and needs no interpreter at runtime:

```
umask 022 && ~/.cargo/bin/cargo build --locked --offline -p cortex-acp -p cortex-paseo-tools
CORTEX_ACP_RUNTIME_ROOT=/absolute/path/to/checkout /absolute/path/to/checkout/target/debug/cortex-acp
```

`cortex-acp` writes only JSON-RPC lines to stdout; every log goes to stderr. Startup schedules the owned pinned-adapter download and native-account setup in a background thread, never on the protocol path.

Worker role instructions are maintained in canonical `skills/worker-*/SKILL.md`
sources and composed with shared protocol text. The package no longer includes
redundant `agents/*.toml` worker profiles or generated copies under
`agents/references/`; `profiles.json` maps role entries to Markdown sources, and
`scripts/cortex-xtask.sh profiles check` validates generated
instruction artifacts. The 23 role instructions remain; the duplicate TOML
serialization format does not.

### Launch-time environment variables

| Variable | Used by | Status |
|---|---|---|
| `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | the runtime and everything built on it (connections, catalog, user defaults) | ACP path, stays |
| `CORTEX_OBSERVATION_DIR` | the runtime/diagnostics logging sink | ACP path, stays |
| `CODEX_HOME` | Native codex-acp/codex CLI isolation directory | ACP path (isolation) |

## Local desktop Debian builds

Run either script from any working directory (the Rust toolchain from `rust-toolchain.toml` and Docker, or Podman,
must be available; the container downloads its locked build dependencies).
The desktop shares Paseo presentation types, so the builder also installs Paseo’s
locked development dependencies, without lifecycle scripts, before typechecking.
These build-time dependencies are not copied from the host. The source snapshot
resolves Rust workspace members from the root `Cargo.toml` (including member
globs and exclusions), rather than maintaining a separate crate list. It copies
`Cargo.lock` unchanged and includes SQL schema assets used by the Rust core.
Missing or linked member manifests fail before the container build; hidden
files, symlinks and dependency/build caches remain excluded:


```sh
./scripts/build-dev-deb.sh
./scripts/build-prod-deb.sh --jobs 6
# Optional container engine:
./scripts/build-dev-deb.sh --engine podman
```

The production wrapper sets `umask 022` and puts `$HOME/.cargo/bin` first in its build environment.
Both wrap `cargo run --locked -p cortex-xtask -- build-deb` (the `cortex-xtask build-deb` driver; `--release`, `--local-release`, `--ci-dev` and `--resume` are reserved by the wrappers) and print the final absolute
archive paths. Copies and their build manifest land in the git-ignored
`.workspace-build/local-dev/build-*/` or `.workspace-build/local-prod/build-*/`.
Each build retains its input snapshot and artifacts under `.workspace-build/build-*/`. Local builds reuse a Cargo target directory, registry and npm downloads under `.workspace-build/.cache/`, separately for each channel and builder image. The running container locks its cache. Unchanged source files retain their Cargo timestamps after byte and mode checks. Each invocation reinstalls the locked npm dependencies and creates a fresh archive and version. Official builds use a cache inside their own build directory. The default Cargo job count follows available CPUs up to six. Use `--jobs N` to override it.
Each invocation uses a fresh build directory; channel flags and `--resume` are
reserved by the wrappers. Other builder arguments pass through unchanged.

Dev uses the ordinary local dev channel. Prod explicitly selects
`--local-release`: a **non-official release-channel build for owner manual
testing only**, refused in GitHub Actions. Its manifest records
`local_release: true` and `owner_handoff_eligible: false`. Rust compilation and
Debian packaging accept exactly `local-owner-test` or `github-main-clean` for
release builds; missing or other authorization values fail. Local production
archive versions end in `.local`, which the unchanged GitHub publisher rejects.
The owner runs the prod wrapper explicitly; it neither publishes
nor installs anything. It uses production package/service/state identities:
installing it can replace an existing production package, so test only in a
disposable environment. The official `--release` clean-main GitHub gate is
unchanged. These scripts do not replace the installed desktop
handoff gate below; pass `--check-installed --prior-archive /absolute/path/to/prior.deb`
to run it after building. Both dev and release channels are supported by the gate.
A local-release build remains non-official even after passing it.

### Build output hygiene

Every successful `build-deb` (and so each wrapper run) ends by keeping only the
newest two `.workspace-build/build-*` directories per channel (`dev`, `release`,
and `incomplete` for builds without a manifest) and the newest two
`.workspace-build/.cache/<channel>-<image>` caches per channel; a removed build
also takes its `local-dev/` or `local-prod/` copy. It prints each removal and the
total freed on stderr, and a hygiene failure never fails the finished build.
Never removed, whatever their age: the build in progress (`build-deb` holds an
exclusive `flock` on its build directory for the whole build), any build or cache
a running Docker or Podman container references (its `container.cid`, or a
bind-mount source), the build holding the exact archive of an installed
`cortex-workspace` or `cortex-workspace-dev` package (`dpkg-query`), the current
build's cache, and a Dev build whose `local-dev` copy may be in use while another
process holds the `local-dev` lock. `--keep-builds N` changes the count (`0`
disables it). Run the same cleanup on demand, or preview it:

```sh
scripts/cortex-xtask.sh clean-builds --dry-run
scripts/cortex-xtask.sh clean-builds --keep 2
```

A container engine that lists running containers but cannot be inspected in time
refuses the cleanup rather than guessing. Cargo's own `target/` directory is never
cleaned automatically. Reclaim it explicitly when needed, for example
`cargo clean --profile dev` (debug artifacts), `cargo clean --release`,
`cargo clean -p <crate>` (one package), `cargo clean --doc`, or plain `cargo clean`
(everything; the next build is a full rebuild).

## Manual Workspace releases

Once `.github/workflows/workspace-desktop-release.yml` (named **Workspace release**)
is present on the default branch `main`, GitHub Actions shows its **Run workflow**
button. Pushes never publish automatically. The workflow has three jobs: `build` (the
`.deb`), `android` (the APK) and `publish`, which needs both and creates one release.

1. Open Actions → Workspace release → Run workflow.
2. Choose branch `main` for production or `dev` for development, then start.
   Other branches and tags are refused; there is no independent channel input.
3. Download the package, the APK and their SHA-256 sidecars from the resulting release.
   Production releases must also include the desktop `.deb.sha256.sig`.
   Main publishes `cortex-workspace_<version>_<arch>.deb` and the release-channel APK
   under `v<debian-version>`, explicitly Latest. Dev publishes
   `cortex-workspace-dev_<version>_<arch>.deb` and the dev APK under
   `dev-v<debian-version>`, marked prerelease and never Latest. Both artifacts and their
   `.sha256` files are attached to the one release.
4. Install dev manually with `sudo apt install ./cortex-workspace-dev_<version>_<arch>.deb`
   after replacing the placeholders with the downloaded filename. Dev has no
   in-app updates. Its separate package, launcher, service and state can coexist
   with production; native engine accounts remain shared in the owner's HOME.

GitHub shows both channels in one repository release list. The production
updater reads only `/releases/latest`; dev prereleases do not replace it.
Existing tags are not overwritten; lookup failures other than HTTP 404 stop
publication. Runs of the same branch are serialized without cancellation.
Publication grants contents-write only to the publish job through GitHub's token.
The `android` job runs `scripts/build-android-apk.sh` (`--release` on `main`, the dev channel on `dev`) on an Ubuntu runner with JDK 17 and the Android NDK of the runner image. It needs repository secrets `ANDROID_KEYSTORE_BASE64` (the keystore, base64) and `ANDROID_KEYSTORE_PASSWORD`, and optionally `ANDROID_KEY_ALIAS` (default `cortex-workspace`); without the first two it fails closed before building, so no release is published. Create the keystore once with `scripts/build-android-apk.sh --init-signing`, which prints these names; an app update installs over an existing one only when signed with the same key. The keystore is removed from the runner after the build, and the job checks the APK against its SHA-256 file. The workflow has not yet been exercised on GitHub, and the Android job has never run.

### Desktop update signing

Before the first production release, initialize the owner key from the checkout:

```sh
public_key=$(scripts/cortex-xtask.sh update-key init) &&
  printf '%s\n' "$public_key" > packaging/update-signing/release-ed25519.pub
scripts/cortex-xtask.sh package stamp
scripts/cortex-xtask.sh package check
```

The command prints only the public key. Commit only that public key and the
refreshed package identity. The private Ed25519 seed is generated once, never
overwritten, and stored at
`~/.config/cortex-workspace/update-signing/release-ed25519.key` (0600, in a 0700
directory). An absolute override via `CORTEX_UPDATE_SIGNING_KEY` must point
outside the checkout with the same permissions. Keep a protected offline backup;
never copy the private file into Git or print it.

To sign and verify a local desktop archive without publishing anything:

```sh
scripts/cortex-xtask.sh update-sign /absolute/path/to/cortex-workspace_VERSION_ARCH.deb
scripts/cortex-xtask.sh update-verify /absolute/path/to/cortex-workspace_VERSION_ARCH.deb
```

Signing creates the standard `.deb.sha256` if absent, checks it against the
exact filename and archive, then writes a raw 64-byte detached signature to
`.deb.sha256.sig`. Verification uses the embedded public key by default. The
optional `--public-key FILE` is for separately trusted keys (for example,
disposable signing fixtures); it cannot change what the application trusts.

For the production workflow, set the repository's Actions secret
`UPDATE_SIGNING_KEY_BASE64` before a future manual `main` run. Feed the raw seed
through stdin rather than pasting or logging it:

```sh
base64 -w0 "${CORTEX_UPDATE_SIGNING_KEY:-$HOME/.config/cortex-workspace/update-signing/release-ed25519.key}" |
  gh secret set UPDATE_SIGNING_KEY_BASE64
```

Grant workflow-write access only to trusted maintainers; control of the signing
secret permits signing arbitrary updates. The signing step runs only on
`main`, decodes the secret under `umask 077` in a private runner-temp directory,
removes it on step exit, and stops before publication if the secret is absent,
invalid or does not match the embedded key. Dev remains manual and does not
require this signing secret. No release has been published to qualify this path.

A missing public key prevents in-app release updates. To rotate a key after
loss or compromise, create a new key at a separate private path, replace the
public key, restamp and rebuild, and replace the CI secret. Installed builds
trust only their original key; distribute the new build by an independently
trusted manual channel rather than expecting those installations to accept a
signature from the new key.

### Local builds of both installables

`scripts/build-local.sh [desktop|apk|all] [--release] [--debug-apk]` builds the desktop package (`build-dev-deb.sh`, or `build-prod-deb.sh` with `--release`) and/or the APK (`build-android-apk.sh`), desktop first, and prints each path. `--debug-apk` signs with Android's debug key so no keystore is needed. `scripts/build-android-apk.sh [--release] [--debug] [--all-abis] [--init-signing]` writes `.workspace-build/android/<package>_<version>_<arch>.apk` and its `.sha256`; its prerequisites and signing variables are in [platforms](../cortex-workspace/platforms.md#android-build-prerequisites). A debug APK was built on the development machine; installing any APK on a device has not been verified.

## Installed desktop handoff gate

A successful build or extracted-binary check is **not** permission to hand the
archive to an owner. First run the [installed real-input gate](verification.md#installed-desktop-real-input-gate).
Use its `--close-cycles 3` option when changing native rendering or shutdown:
every clean/upgrade phase must close and reopen repeatedly, while the independent
service stays available and native allocator/crash diagnostics remain fatal.
The Linux view keeps compositing available and terminates its own WebKit web
process only after guarded, acknowledged closure. Software-display success is
not real-GPU performance evidence.
The exact archive must pass on both X11 and Wayland after clean installation and
ordinary `apt install` upgrade from the previous owner archive. Run sequentially:

```sh
scripts/cortex-xtask.sh build-deb --check-installed --prior-archive /absolute/path/to/previous-owner.deb
# Or check an already-built, unchanged archive:
scripts/cortex-xtask.sh installed-gate --archive /absolute/path/to/candidate.deb --prior-archive /absolute/path/to/previous-owner.deb --evidence .workspace-build/installed-new
# Release channel (both candidate and exact prior archive must be cortex-workspace):
scripts/cortex-xtask.sh installed-gate --channel release --archive /absolute/path/to/candidate-release.deb --prior-archive /absolute/path/to/previous-release.deb --evidence .workspace-build/installed-release-new
```

The evidence path must not exist. Keep its complete output and `result.json`;
`accepted: true` must bind to the SHA-256 of the actual archive handed over.
The builder records `owner_handoff_eligible: false` without this gate.
A failed, incomplete or single-display run never qualifies. The harness requires
Docker. `--channel dev` (the default) checks `cortex-workspace-dev`; `--channel release`
checks `cortex-workspace`. Mixed-channel archives are rejected before any container
launch. Both use the same four scenarios and all existing real-input checks, with
channel-specific service, socket, store, config and state paths. Release checks run
only inside disposable containers, never against the installed production app.

Every scenario seeds synthetic orchestration files before installation. They contain
six profiles, active `default`, and an exact user-default coordinator route with
`thought_level: medium`, not real accounts or secrets. The gate compares byte and
semantic SHA-256 values after install, before/after upgrade where applicable, and
after the first GUI launch. Settings must render the six profiles with `default`
active and the coordinator model and effort, including with an unreadable registered
project. The synthetic files are retired only after those checks, before unrelated
fake-engine journeys. No workflow publication or host installation is performed.

### Native service startup

The service sets `CARGO_BUILD_JOBS` and `CMAKE_BUILD_PARALLEL_LEVEL` to half of the machine's CPUs (at least two) when the owner has not set them, before it starts anything, so every engine and task command inherits a bounded build parallelism; the owner's own value always wins. Both shipped user units run with `CPUWeight=20` and `MemoryHigh=60%`. Every task process (engines, builds, tests, renders) lives in the unit's cgroup (`KillMode=control-group`), so under contention the desktop session gets the CPU first and, above the memory threshold, the kernel reclaims the unit's own memory instead of swapping out the session; nothing is killed for it (no `MemoryMax`), and an idle machine runs tasks at full speed. `IOWeight` is not set: a user manager is not delegated the `io` controller. Both units also set `IOSchedulingClass=best-effort` with `IOSchedulingPriority=7`, the lowest best-effort priority, so task disk access yields to the session without starving on a block device whose scheduler honours I/O priorities (BFQ); with the `none` scheduler it has no effect. Both shipped user units invoke `workspace-service <allowed-root> <runtime-root>` (the installed binaries `cortex-workspace-service` and `cortex-workspace-service-dev`, allowed root `/`). The runtime root is a trusted absolute source checkout or this channel's installed `/usr/lib/<package>/runtime` directory. The only other accepted forms are the one-argument MCP relay and the internal maintenance-worker mode: the former three- and five-argument forms, `--fixture-launcher`, the retained/legacy ACP child and `CORTEX_WORKSPACE_NATIVE=0` were removed with the Python runtime. Binding the socket and serving native operations requires no interpreter or helper files, and the `.deb` depends on no `python3` and ships no `src/` or `.py` files. Native setup/login bundle discovery and Node voice assets derive from the trusted runtime root. Requests that only the former Python helper could answer are refused with `config_api_unavailable` or their native refusal code, never forwarded. See [the startup boundary](../cortex-workspace/local-task-execution.md#python-independent-native-service-startup). The installed-archive real-input gate above remains mandatory before handoff; source/socket tests do not establish rendered desktop or installed-upgrade results.

## Offline Workspace engines

The package content identity (`package-identity.json`, written by `scripts/cortex-xtask.sh package stamp` and validated by `package check`) covers the exact shipped runtime file set listed in `runtime-payload.json` (`scripts/cortex-xtask.sh runtime-payload list`). That set contains no Python and no `src/` tree. Changing a payload file requires restamping before packaging.

Both Debian channels bundle the qualified adapters, their dependency tree and Node
under `/usr/lib/<package>/engines`. The container installs using the committed
`packaging/engines/package-lock.json`, verifies each required native component,
and checks Node's committed archive checksum and equality with the builder's Node.
The package build fails if any inventoried file is missing or modified, or the
lock differs from `pinned-dependencies.json`. Nodejs/npm are not Debian dependencies.

Discovery uses only the bundle belonging to this runtime's channel, prefers it to
user npm installs, and retains canonical-path install verification and exact instruction
registration checks. The Codex patch is already prepared in private connection
state and links read-only dependencies; nothing writes under `/usr/lib`.
Same-version connections may relocate to the bundle, but never downgrade.
A working bundle makes the optional npm updater a no-op. Checkout installs retain
the existing bounded optional npm download; absent npm stays idle.
Native accounts remain in `~/.codex` and `~/.claude`, never copied; OpenCode
remains optional. Installation without network does not make model use offline.

[Licence inventory and owner distribution-risk decision](bundled-engine-licenses.md).
The no-paid real launch check is:

```sh
scripts/cortex-xtask.sh check-bundled-engines --archive /absolute/path/to/package.deb
```

It prepares apt libraries in a clean Ubuntu 26.04 image, then installs the archive
in a new `--network none` container with no system Node/npm. As a normal user it
uses real discovery, registration, install resolution, EngineProcess and ACP
initialize, checks native binaries' versions, and verifies the root-owned bundle
remains unchanged. No login, session/new, model prompt or paid call is made.
This does not qualify sandbox policy, systemd startup or rendered desktop behavior.

## Pinned dependencies

`./pinned-dependencies.json` is the single manifest for:

- this package's own portable contract version;
- the vendored ACP schema fixture's pinned sha256 (the manifest path field still reads
  `tests/fixtures/acp/schema-1.4.0.json`; the vendored schema used by the Rust tests is
  `crates/cortex-acp/tests/fixtures/acp/schema-1.4.0.json`);
- each supported engine adapter's pinned version, the current qualified
  adapter (codex-acp 2.6.0, whose `@openai/codex` dependency is `^0.166.0`;
  claude-agent-acp 0.107.0). Settings' **Download engine** and the background
  updater both install exactly this pin; see
  [codex-acp-adapter.md](../features/codex-engine/index.md) and
  [claude-agent-acp-adapter.md](../features/claude-engine/index.md) for the
  verified-digest install-record mechanism and the other qualified versions.

`cortex-acp` reads the manifest at startup (portable contract version and manifest schema) before it
constructs the runtime, and exits 1 with a concrete stderr message on any mismatch; stdout stays
empty. No interpreter floor exists. `scripts/cortex-xtask.sh check-pins` verifies the reviewed
Rust/CLI dependency pins without a build or network.

## First-run setup

`scripts/cortex-paseo-tools.sh` (the `cortex-paseo-tools` binary) has independent, idempotent, non-destructive steps (`status` only reads). Every command prints one JSON document with sorted keys; a refusal prints `error: ...` on stderr and exits 1:

```
scripts/cortex-paseo-tools.sh status
scripts/cortex-paseo-tools.sh init-storage
scripts/cortex-paseo-tools.sh install-engine --engine codex-acp
scripts/cortex-paseo-tools.sh add-connection --engine codex-acp
scripts/cortex-paseo-tools.sh validate-connection --connection cn_xxxxxxxxxxxx
scripts/cortex-paseo-tools.sh pin-route --connection cn_xxxxxxxxxxxx --model gpt-5-codex
scripts/cortex-paseo-tools.sh refresh-catalog [--workspace /path/to/project]
scripts/cortex-paseo-tools.sh print-client-recipes
scripts/cortex-paseo-tools.sh install-paseo-plugin
```

- **`init-storage`** creates/verifies the owner-private
  `$XDG_CONFIG_HOME/cortex`/`$XDG_STATE_HOME/cortex` roots (`0700`,
  symlink-refusing) and installs the absent default preset (it calls the native settings API's `apply_default_preset`). It never touches
  any project's own `.cortex` store.
- **Default preset seeding.** `init-storage` creates absent `routing.json`,
  `defaults.json` and `mcp-servers.json` files, plus one applied `default` profile in
  an absent `routing-profiles.json` when the working settings match the preset. MCP
  entries are disabled examples (`codebase-memory-mcp`, `playwright`, `vivaldi`),
  not downloaded or launched executables. All paths are `/path/to/...` placeholders.
  See [Default preset](../configuration.md#default-preset).
  An existing file is never replaced. Every Cortex agent start repeats this absent-only seeding.
- **`install-engine --engine ENGINE`** installs the manifest-pinned adapter into Cortex's owned engine root (an explicit opt-in).
- **`add-connection --engine ENGINE`** registers (or reuses) one native-account connection through the
  native settings API (`onboard_connection`, with an explicit opt-in and the current registry revision). It
  never collects or stores a credential: the engine's own login runs entirely outside Cortex.
  The former API-key route is gone: `--api-key-file`, `--api-key-stdin` and `--install-record` are refused with an explanatory error.
- **`add-opencode-connection [--binary PATH] [--name NAME]`** onboards the optional
  OpenCode v2 engine, which is never discovered at startup. Its version probe must run in the
  OpenCode network-less sandbox, which the Rust host does not provide, so it currently refuses with
  `config_api_unavailable` (see the [OpenCode adapter](../features/opencode-engine/index.md#onboarding)).
- **`validate-connection --connection ID`** recomputes one connection's current state (the settings API's `validate` update, revision-checked)
  and prints only its id, engine, state, and qualification flag. It never prints
  credential paths or values.
- **`pin-route --connection ID --model MODEL`** persists an observed catalogue route as the user-default route
  (native catalogue only: the route exists once a real engine session observed it, see `refresh-catalog`). A fresh task's
  first prompt then adopts this default automatically, with no further per-task action.
- **`refresh-catalog [--workspace PROJECT]`** refreshes the model catalogue through the settings API (`route_catalog` `refresh`); it replaces the removed manual `record-observation` step.
- **`print-client-recipes`** prints the Zed entry and the codeg recipe text; it is a dry run by default and a real
  write needs `--settings PATH --apply --i-consent-to-write` (see [Zed](../features/zed-agent/index.md)).
- **`install-paseo-plugin`** and **`configure-paseo-plugin-launch`** are described under [Paseo provider plugin](#paseo-provider-plugin).

## Diagnostics

The Python diagnostics report (`cortex_diagnose`) was removed on 2026-11-16 without replacement. Read-only checks that remain, none of which makes a model call:

```
scripts/cortex-paseo-tools.sh status                  # roots, connections, discovered engines
scripts/cortex-xtask.sh check-pins                    # reviewed dependency pins, no build, no network
scripts/cortex-xtask.sh prerequisite-probe            # inventory of native-build prerequisites
scripts/cortex-xtask.sh package check                 # package identity stamp
scripts/cortex-paseo-tools.sh acp-smoke               # startup smoke in disposable HOME/XDG roots
```

The paid probes (`probe-codex`, `probe-claude`) stay entirely separate: they refuse to run at all without both `--i-authorize-paid-call` and `--approval-ref`, and none of the commands above reaches them.

## Retention maintenance

The packaged Workspace service also runs automatic retention without any command: on project activation and then hourly it archives old Done tasks and, unless `automaticArchivePurge` is switched off in Storage, deletes expired closed archives. It touches no active or unclosed task, dirty or unmerged worktree, migration backup or qualification evidence. Settings and defaults: [configuration](../configuration.md#workspace-storage-retention); rules: [storage](storage.md#automatic-workspace-retention). The `/cortex-clear` text below is the separate ACP chat route.

Use `/cortex-clear` in an idle ACP session to preview old task deletion from
this project. The native host does not yet delete: a confirmed `/cortex-clear` that would delete something refuses and writes nothing (see [known limits](native-incoming-acp.md#known-limits)); the confirmed-deletion behavior below is the contract carried from the former implementation. It is advertised in the ACP command picker and handled by the
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
The manual `cortex_clear` CLI (a Python script that deleted directly with `--days`, `--keep-thread`, `--force` and `--dry-run`, run only by the owner while the project runtime was idle) was removed on 2026-11-16 without replacement. There is no supported out-of-chat retention command today; do not delete task directories or `pending_deletions` rows by hand. Retention semantics (activity is the maximum of task creation and published report/pipeline edition update times; cleanup removes project task directories and unambiguously associated drafts; transactional `pending_deletions` intents remain recoverable after interruption) are those of the chat command above.

## Uninstall

`scripts/cortex-paseo-tools.sh uninstall` is dry-run by default (prints exactly what
would be removed, deletes nothing). Real deletion needs an explicit
`--remove-user-scope` and/or `--remove-project-store --project PATH`,
together with `--i-confirm-removal`:

```
scripts/cortex-paseo-tools.sh uninstall                                   # dry run
scripts/cortex-paseo-tools.sh uninstall --remove-user-scope --i-confirm-removal
scripts/cortex-paseo-tools.sh uninstall --project /path/to/project \
    --remove-project-store --i-confirm-removal
```

It deletes only `$XDG_CONFIG_HOME/cortex`, `$XDG_STATE_HOME/cortex`, and
(only when `--project` is given) that one project's own `.cortex` store —
each re-verified as an owner-private, non-symlinked real directory
immediately before deletion. It never touches engine installs, real
credentials outside those Cortex-owned roots, user history, project files
other than `.cortex`, or Git branches/working changes
(the `owner_tools` unit tests in `crates/cortex-paseo-tools` check the removal refusals and the project-store removal).

## Zed and codeg registration

See [zed-custom-agent.md](../features/zed-agent/index.md) and
[codeg-custom-agent.md](../features/codeg-agent/index.md) for the full
recipes; `scripts/cortex-paseo-tools.sh print-client-recipes` prints both
without writing anything unless explicitly told to.

### Paseo provider plugin

Run one explicit setup command:

```bash
scripts/cortex-paseo-tools.sh install-paseo-plugin
```

The plugin registers a Cortex Settings screen for supported owner/project configuration. Its values are stored by the Cortex runtime (JSON files or the existing project SQLite configuration row), not in Paseo's settings document. A live Paseo 0.10.0 check did not find the screen in the rendered Settings navigation or command palette, so do not assume it is reachable in that host version. See [configuration management](../configuration.md#manage-settings-in-paseo) for scope, revision conflicts, redaction and the OpenCode opt-in. Its Connections section can also download Cortex's pinned `codex-acp` or `claude-agent-acp` adapter and add a native-account connection after an explicit opt-in; see [Engine install records](../configuration.md#engine-install-records).

#### Install from GitHub

The plugin lives below the repository root, so include the Paseo subdirectory
suffix. Paseo stages the full Git checkout and runs the manifest's supported
`build` command before activation. The manifest
(`integrations/paseo/paseo-plugin.json`) runs
`cargo build --release --locked -p cortex-acp -p cortex-paseo-tools` and then
`cortex-paseo-tools bootstrap-paseo-git-install`, which prepares the
private runtime (the non-code payload, the load shim and the two built binaries) and launch record. The record names the staged
Rust `cortex-acp`. It runs `init-storage` itself, so the default preset
files are seeded when absent. No separate `init-storage` or
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

Paseo stages the complete repository; the
`:integrations/paseo` suffix selects the plugin entry directory. The build
step retains the launch-critical runtime under
`$XDG_STATE_HOME/cortex/paseo-runtime/` and writes a `0600` launch record at
`$XDG_CONFIG_HOME/cortex/paseo-plugin-launch.json`, or
`~/.config/cortex/paseo-plugin-launch.json` when `XDG_CONFIG_HOME` is unset.
The build step runs in the Paseo daemon's environment, so a second Paseo
instance with its own `XDG_CONFIG_HOME` writes and reads its own record and
never replaces the record of the conventional installation. An existing record
that Cortex did not write is still refused, never overwritten.

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
[failures and disconnects](../features/failures-and-cancellation/index.md#4-disconnect)). Until
that path is qualified in a live dev Paseo run, update between active tasks.

The managed local-checkout setup remains available while Paseo is running:

```bash
cargo build --locked --offline -p cortex-acp -p cortex-paseo-tools
scripts/cortex-paseo-tools.sh install-paseo-plugin
```

It validates and installs/reloads the repository-owned provider, then verifies
that it is running. This is the supported explicit local setup workflow. Its
launch record names this checkout's built Rust `target/debug/cortex-acp` by
default and refuses before any daemon change when it or `cortex-paseo-tools` is
not built; `--rust-binary` names another binary and `--source-root` another checkout.

`install-paseo-plugin` is the single explicit Cortex-on-Paseo installation
step. It validates the repository-owned `integrations/paseo` source and its
generated SHA-256 integrity record, enables the Paseo daemon plugin switch
only for this explicit setup action, installs or reloads the `cortex-acp`
provider, and verifies that the daemon reports it as `running`. It is
idempotent and preserves unrelated Paseo plugins and configuration keys.
Setup also writes the owner-private `paseo-plugin-launch.json` record with the
absolute Cortex entry (the Rust binary by default) and the non-secret `HOME`, `XDG_CONFIG_HOME`, and
`XDG_STATE_HOME` roots used by setup. The record lives in the Cortex config
root of that environment: `$XDG_CONFIG_HOME/cortex/` when `XDG_CONFIG_HOME` is
set to a directory other than `~/.config`, otherwise `~/.config/cortex/`. The
provider looks first in its own `XDG_CONFIG_HOME` and then in `~/.config`; a
daemon with its own `XDG_CONFIG_HOME` uses the `~/.config` record only if that
record names the same `XDG_CONFIG_HOME` (a record written by an older setup),
so it never launches another installation's runtime and storage. Run setup in
the same XDG environment as the daemon: a daemon started without
`XDG_CONFIG_HOME` does not see a record written under a custom one. The
provider applies the recorded roots when it starts Cortex after a Paseo daemon restart, so the same connection
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
The plugin installation command never runs implicitly during an ACP session. Agent starts
separately seed missing default-preset files and follow approved adapters. Plugin activation
and agent startup also start one non-blocking background pass: install each qualified
manifest pin, register a missing native-account connection only when its login marker
exists, then load its models without a prompt. Owner-removed engines stay opted out
of automatic registration. Settings polls progress and keeps manual download/connect
and model-refresh actions available. Concurrent startup passes share an owner-private
lock; npm installs are bounded to 600 seconds each (manual download: 60 seconds),
model-discovery RPCs to 20 seconds each. A model-discovery failure keeps the
connection; its per-engine status (`first-run-<engine>.json` in the Cortex config
root) and Settings name a fixed failure code such as `engine_home_read_only`
(the engine's account directory, for example `~/.codex`, is read-only for the
Cortex process; Codex cannot start then) or `engine_unreachable:<error type>`,
never adapter output. A stopped pass retries at the next start;
`CORTEX_ENGINE_UPDATE=off` disables the automatic pass. See
[configuration](../configuration.md#approved-adapter-following-and-rollback). After installation,
Paseo exposes the native Cortex provider; no separate generic ACP provider
registration is required for Paseo. The source hash is regenerated by
`scripts/cortex-xtask.sh package stamp` and checked by `package check`. Paseo plugin code is trusted, unsandboxed daemon code, so this
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

### Paseo update and existing MCP registries

`scripts/update-paseo-plugin.sh` builds the Rust entries (`cargo build --locked --offline -p cortex-acp -p cortex-paseo-tools`), stamps the package and calls `install-paseo-plugin` (the built Rust entry is required; set `CORTEX_PASEO_CLI` when `paseo` is not on `PATH`), which validates the
owner MCP registry before changing the daemon or launch record. A malformed or
pre-gateway entry without exact `allowed_tools` fails the update preflight with
its server name and required field. No missing grant is converted into wildcard
access. Repair enabled entries using the installed catalogue, explicit binary and
dependency `read_paths`, and only the required network permission.

An already-running `cortex-acp` plugin is now explicitly reloaded through Paseo's
installed `plugin reload` command. This restarts its provider process, so perform
updates between active tasks. A plugin's `running` state alone does not certify
MCP operations; retained startup and real gateway checks are recorded in the
production repair observation.

## Model-policy installation and explicit migration

The public policy/model-settings-default.json is an installation-only, schema-validated candidate. It is packaged with the runtime, but runtime routing never reads or merges it. Fresh setup creates only absent settings files with private no-replacement writes; existing, partial, legacy owner settings and saved profiles remain unchanged. The candidate is derived from retained production configuration and observed route policy, not dev/example files. Its portable coordinator preference contains engine/model/parameters, not machine-specific route, connection or account identifiers. Installation is not evidence that production was updated or a live route qualified.

For an existing installation, derive a candidate from owner settings preserving explicit denies, providers, preferences and dormant policy. Run preview, review the diff/diagnostics and source/candidate digests, then apply only with explicit consent and the exact preview revision/digest. The tool verifies a private backup before replacement, verifies new bytes and attempts exact rollback on failure; rollback failure is explicit. Migrate each saved profile separately from that profile's own candidate. Do not apply a working-settings candidate to inactive profiles. Portable coordinator binding is accepted only for one exact currently selectable catalog identity and requested parameter set; unavailable or multiple matches require owner resolution, never automatic substitution.

The former command-line tools for this flow (`cortex_model_policy_migrate preview/apply` and `cortex_routing_preview_check`) were Python and were removed on 2026-11-16 without replacement. The supported route is the settings API (`cortex-paseo-tools config` and the Paseo Settings screen), which revision-checks every write and reports the diff and diagnostics. Applying a candidate is a configuration mutation and requires explicit owner authorization. The retained narrative below describes the former tools and is history. Apply is a configuration mutation and requires explicit owner authorization. Saved-profile deletion is separate and revision-checked: confirmation identifies the exact profile; cancel changes nothing; deleting the selected profile clears its linkage without resetting materialized settings; deleting the last profile is permitted. Errors preserve profile/settings and require explicit reload before retry.

### Routing-only assertion preview (removed tool)

The former bounded, nonmutating check of current owner routing/default files (`cortex_routing_preview_check`, removed) checked owner-private regular non-symlink files, a bounded allowlisted non-secret routing schema and coordinator/parameter shape; completes only missing fields from reviewed public installation data; asserts preservation and agreement between the migration API and real CLI preview; and verifies exact before/after bytes. It reads only routing/default files, not profiles, registry, accounts or logs. Unknown top-level routing keys refuse this narrow utility rather than being copied into a candidate; the general migration tool retains unknown owner fields and requires a purpose-specific owner candidate. Existing tier/list ordering is retained. The temporary candidate is private and deleted on exit; the output contains full hashes and assertion outcomes, not private identifiers or settings bodies. No apply is performed.

The apply command above holds the owner-private `connections.lock`, creates and verifies a new 0600 backup under `.migration-backups/`, and atomically replaces routing with 0600 bytes. Defaults and saved profiles are outside a routing-only apply. Source or candidate drift requires a new preview. Automatic failure rollback verifies the original bytes; there is no public CLI action for restoring a backup after a successful migration. Such a later restore needs a separately authorized, lock-protected operation with current-source and backup-digest preconditions; do not use migration apply to bypass preservation checks. A preview does not qualify production adoption, live quota labels or an installed runtime/plugin.
