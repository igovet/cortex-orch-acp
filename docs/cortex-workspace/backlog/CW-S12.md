# CW-S12: Isolated dev and release channels, dev settings clone and per-project channel stores

**Status:** implemented; installed-runtime and user-manager qualification limits are in [platforms](../platforms.md); the release channel is not published · **Phase:** 3+ · **Size:** M

## Goal

Keep the development build and the GitHub release build apart. A local build is the dev channel: it has its own package, binaries, user unit, runtime, configuration, state and socket namespace, so it cannot read or change the production Cortex stores. Manual GitHub Actions runs on `main` build the release channel; runs on `dev` publish dev prereleases excluded from Latest (owner decision 2026-11-06). The owner can copy non-secret Cortex settings into the dev configuration, and the Workspace never adopts a project store it does not own.

## Requirements

CW-R42 (isolated channels, dev settings clone and per-project channel stores), CW-R16 (protect credentials, project paths and device identities) and CW-R38 (the update path follows the channel rule). See [requirements](../requirements.md).

## Inputs

- [CW-S11](CW-S11.md): the `.deb` packaging, the user units and the release workflow.
- [CW-U16](CW-U16.md): self-update, which must respect the channel rule.
- [roadmap](../roadmap.md): owner decisions of 2026-11-05 on channels and the container build.

## Deliverables

- `crates/workspace-protocol/channels.json`: the single table of channel identities. Dev and release each name their package, Tauri identifier, title, installed binary names, user unit, and runtime, configuration and state namespaces. The dev channel uses the `cortex-workspace-dev` names throughout.
- `crates/workspace-protocol/build.rs` and `src/channel.rs`: the channel is compiled into the shared crate. Dev is the default. An unknown channel fails the build, and the release channel requires the guarded-build authorization marker.
- `scripts/build_workspace_desktop.py`: builds in the container with the dev channel by default. `--release` requires a manual GitHub Actions workflow on clean `main`; `--ci-dev` requires a manual workflow on clean `dev`.
- Service: both channels ship the complete ACP payload and Workspace host below `/usr/lib/<package>/runtime`. The unit uses that channel-owned path without a checkout or `service.env`; stale environment files are ignored. A developer can still explicitly launch a source checkout, and the named fake-engine fixture route remains available. Dev never falls back to the production runtime.
- Dev child environment: the ACP child's XDG config, state, cache, data and runtime directories point to dev-only paths. `HOME` is not changed and no account or credential file is copied.
- Dev default paths: configuration in `$XDG_CONFIG_HOME/cortex-workspace-dev/cortex` (with `~/.config` as fallback); registry in `$XDG_STATE_HOME/cortex-workspace-dev/projects.json`; socket and lock below `$XDG_RUNTIME_DIR/cortex-workspace-dev`.
- Project channel stores: release/default ACP uses `<project>/.cortex`; Workspace Dev uses `<project>/.cortex-dev` beside it. A release store never prevents opening the project in Dev and is never read, changed, imported or migrated by Dev. Registration creates the private `.cortex-dev` directory and its `.cortex-dev/owner.json` reservation together, binding its device/inode before any database opens. Existing validated root `.cortex-dev-owner.json` reservations migrate once to this location. Recognized private `.cortex-workspace-dev.json` markers move to `.cortex-dev/legacy-workspace.json`; unknown, linked or foreign files stay untouched. No old `.cortex` database is opened or imported. An activated older project with no dev store gets a fresh reservation on restart; an existing unreserved/replaced store stays refused.
- One database and document root per logical project per channel, including linked worktrees. Storage selection is the fixed host channel, not a client-supplied path. Existing format-11/12 migration and inspection rules apply only to that selected store; no cross-channel fallback. Git helpers, snapshots, private-path checks, runtime tools and project routing use the same selection and exclude both stores. Git excludes cover both stores and ownership markers.
- `scripts/clone_cortex_dev_settings.py [--source DIR] [--target DIR] [--overwrite]`: copies only `connections.json`, `routing.json`, `routing-profiles.json`, `mcp-servers.json` and `defaults.json`. Its defaults are the production configuration directory (`$XDG_CONFIG_HOME/cortex`) as source and the dev configuration directory as target. It drops secret-like keys and executable, endpoint, URL and history fields, keeps native account references, leaves connections awaiting authorization and disables MCP servers for review. It refuses symbolic links, overlapping production and target paths, and an overwrite without `--overwrite`. It never reads account or state files.

## Acceptance

Positive:

- Dev and release install paths are disjoint, including the complete runtime, desktop entry, user unit and login-start symlink. First-run setup installs qualified adapters into channel-owned data directories and references existing native logins without a settings clone.
- The native fake-engine journey runs in the dev channel, and its window title is `Cortex Workspace Dev`.
- The clone leaves every production fixture file byte-identical before and after the run.
- A folder containing `.cortex` registers and activates in Dev with a separate `.cortex-dev`; the release fixture remains byte-identical and no release locks, sidecars or migration backups appear.

Negative:

- A release build is refused outside a manual GitHub Actions workflow on clean `main` and without the guarded-build marker; a CI dev request likewise requires clean `dev`.
- An unowned existing `.cortex-dev`, a symlink or changed bound inode is refused before any store changes. Release `.cortex` is not an error.
- The clone refuses symbolic links, overlapping paths and an overwrite without consent.
- A dev service refuses an unrecognised runtime directory, including the release channel's bundle.

## Dependencies

[CW-S11](CW-S11.md).

## Validation

- No-paid tests: `tests/test_workspace_channels.py`, `tests/test_workspace_clone_dev_settings.py` and `tests/test_workspace_container_build.py`. The Rust service and protocol tests run with `cargo test --locked --workspace` inside the container build.
- Container build: `python3 -B scripts/build_workspace_desktop.py` for the dev channel, which requires Docker. Retain the exact command, cwd and exit code.
- Native fake-engine journey and visual audit: `python3 -B packaging/container/check_rendered.py --container --artifacts <build>/artifacts` after the container build. It records text-only outcomes, not screenshots, and fails on any visual regression it measures.
- Unverified: the GitHub release channel has not been published; the clone has not been run against the owner's production settings.

## Documentation impact

- [architecture](../architecture.md), [security](../security.md), [platforms](../platforms.md), [roadmap](../roadmap.md) and the root README and SECURITY pages, once the channel behaviour is part of the shipped slice.
