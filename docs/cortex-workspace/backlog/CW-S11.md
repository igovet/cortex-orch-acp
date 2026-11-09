# CW-S11: Local packaging, user service unit and release handoff

**Status:** implemented; user-manager and installer qualification limits are in [platforms](../platforms.md); GitHub release run not performed · **Phase:** 3+ · **Size:** M

## Goal

Package the local slice for Ubuntu: a `.deb` containing the desktop application, service and complete Cortex runtime, plus an automatically started user-level service unit. Define the handoff to the release checks.

## Requirements

CW-R12 (Ubuntu first) and CW-R13 (accepted desktop direction). See [requirements](../requirements.md).

## Inputs

- [CW-F01](CW-F01.md): the `.deb` output and the package version source, which is the GitHub release tag.
- [CW-U16](CW-U16.md): the later update path. This item does not implement updates.
- [packaging](../../project/packaging.md) and [platforms](../platforms.md).

## Deliverables

- `packaging/systemd/user/cortex-workspace.service`: foreground service, `Restart=on-failure` with a bounded restart delay, `KillMode=control-group`, a 30-second stop timeout, and no linger or boot-survival promise.
- `.deb` inputs with fixed channel-derived paths, including the application, service, complete stamped ACP payload and Workspace host. Runtime dependencies include Python 3.11+, Git, Node.js 20+, npm, bubblewrap, systemd, the user session bus and ELF-derived libraries; development packages are not direct dependencies.
- Installing the package ships a `default.target.wants` symlink for every user's next login. On first launch in an existing session, the native bridge runs only absolute `/usr/bin/systemctl --user daemon-reload` and `start` for its fixed channel unit, with bounded waits and retry. It never replays a failed Workspace mutation. Maintainer scripts reload and try-restart active user units on upgrade, best effort, and stop on removal without deleting user data. No linger is enabled.
- Payload hash regenerated with `python3 -B scripts/cortex_package.py stamp` after any payload edit, before release-sensitive checks. Version 1.0.6 is retained.
- Release handoff: `.github/workflows/workspace-desktop-release.yml` runs only through Run workflow on `main` or `dev`. It builds in the shared container and publishes `v<debian-version>` as Latest for production, or `dev-v<debian-version>` as a dev prerelease excluded from Latest. Both channels appear in one release list; dev installs manually without in-app updates. Publication requires the channel's tag to be absent. A tag lookup error other than not-found stops the release. The workflow has not been run on GitHub.
- Channel isolation and the settings clone are specified in [CW-S12](CW-S12.md).

## Acceptance

Positive:

- Archive tests compare every runtime file with the authoritative ACP payload enumeration, reject a missing required asset or stale content stamp, and verify normal-user-readable file modes.
- Stopping and starting the unit behaves as specified on a reachable user manager. Closing the application leaves the service running.

Negative:

- A protocol major-version mismatch reports "upgrade required" and does not start work.
- A package rollback does not silently resume an execution.
- No claim of boot survival or active-user-manager qualification from a direct ExecStart smoke.

## Dependencies

[CW-S10](CW-S10.md).

## Validation

- Targeted no-paid tests for the unit file and the install paths.
- `python3 -B scripts/cortex_package.py stamp`, then package, profile and test checks run sequentially in the same checkout.
- `systemd` checks are unverified unless a user manager is reachable. Record that; do not substitute a fabricated receipt.

## Documentation impact

- README, [SECURITY](../security.md) (repository root and the Workspace copy where it applies), [platforms](../platforms.md) and [roadmap](../roadmap.md), once the behavior ships.
