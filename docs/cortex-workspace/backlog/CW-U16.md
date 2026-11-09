# CW-U16: Desktop application self-update from GitHub releases

**Status:** Partial source implementation; no privileged update qualification · **Phase:** 3+ · **Size:** L

The built Updates page and checksum-only release workflow are described in
[desktop maintenance and controls](../maintenance-and-controls.md). The proposed
independent signature, root-owned staging policy, automatic health/restart and
schema-compatible rollback qualification below remain open; fixture results do
not close those requirements.

## Goal

Let the Ubuntu desktop app, shipped as a `.deb` package, detect a newer release published on GitHub, show the owner what is new, and update itself after the owner confirms. Every downloaded package is verified before any install step runs. Failure, verification failure and rollback are shown as explicit states. The update covers the desktop app and the local machine service installed with it. Each machine updates its own installation; a remote machine is not updated from another machine.

## Requirements

- CW-R38: `.deb` packaging, release detection, owner-confirmed install, signature and checksum verification, and explicit failure and rollback states.
- CW-R06: the machine service survives window closure and reconciles crashes without replaying prompts; an update must not stop running workers without owner action.
- CW-R12: Ubuntu is the first qualified platform.
- CW-R16: credentials and device identities are protected; unverified or unauthorized packages are rejected.
- CW-R14: download and install budgets are measured, not claimed.

See [requirements](../requirements.md).

## Inputs

- [Platform policy](../platforms.md): the first release targets the Ubuntu machine and the desktop client.
- [Risks and open decisions](../risks-and-open-decisions.md): release distribution and upgrades need owner decisions before operation. This item records the options and recommendations; it does not record the decision.
- [Migration and rollback](../migration-and-rollback.md): the rules for stored data. A version rollback is offered only after those rules confirm that the stored data is compatible with the older version.
- [CW-F01](CW-F01.md): produces the `tauri build` package and fixes the toolchain. Its package output is the input to this item.
- [CW-Q04a](CW-Q04a.md): clean Ubuntu installer checks. Update-path checks are added there.
- [Packaging](../../project/packaging.md): packaging of the current Cortex runtime. It does not describe the desktop app and is not changed by this item.
- Tauri updater: the Tauri 2 updater plugin is a candidate for the download and signature step. Whether it can install a Linux `.deb` must be verified against the version pinned in CW-F01 before it is chosen. This item does not assume it.
- Owner input (2026-11-05): the app is built as a `.deb` package and must notice a new GitHub release and update itself.
- Channel rule (owner decision of 2026-11-05, [CW-S12](CW-S12.md)): the dev and release channels are isolated. A local build is the dev channel. The release feed carries only packages that the release workflow builds from a manual workflow on `main`, so a dev build neither checks that feed nor installs from it, and the release channel does not update from a dev package.

## Open decisions and recommendations

Recommended options are marked ★. The owner decides each one.

1. **Distribution.** (a) ★ Direct download of the signed `.deb` from the GitHub release, installed with `apt-get` so dependencies resolve. One signed artifact per release, and no repository to host. (b) A signed apt repository published on GitHub Pages. It gives native apt upgrades but needs repository key management and a publish step for every release. Recommendation: (a) for version 1; revisit (b) when several machines need it.
2. **Privilege elevation.** (a) ★ A polkit action run through `pkexec`, which calls one fixed helper. The helper installs one verified `.deb` from a root-owned staging folder with `apt-get install`. The package ships the polkit policy. (b) A terminal `sudo` prompt, which the desktop app cannot drive. (c) A resident root service installed with the package, which widens the attack surface. Note: `apt-get install ./file.deb` resolves dependencies; `dpkg -i` does not.
3. **Signature scheme.** (a) ★ A detached Ed25519 signature over the `.deb` checksum file, with the public key embedded in the app and the private key kept offline by the owner. (b) A GPG-signed checksum file. (c) GitHub artifact attestations, which need a separate verifier. Recommendation: (a), which needs no external tool at install time.
4. **Consent.** (a) ★ The owner confirms each install. The privilege prompt already requires a user action, so the confirmation adds no new step. (b) Background download with install at owner confirmation. (c) Silent install, which is not recommended for version 1. Recommendation (a) departs from a fully automatic reading of the owner's request; the owner decides, and (b) can follow.
5. **Rollback.** (a) ★ Keep the previously installed `.deb` in the app data folder until the new version passes its start check. If the new version fails that check, offer "Reinstall previous version". (b) No in-app rollback; a manual apt downgrade. Recommendation (a), subject to the stored-data rule in Inputs.
6. **Release channel.** (a) ★ Stable releases only in version 1. (b) Pre-releases, opt-in per machine.

## Deliverables

- Release check: queries the GitHub release feed of the owner-configured repository at start, on "Check now", and otherwise no more than once a day (proposed default). It compares the published version with the installed one, reads the release notes as plain text and records the time of the check.
- Download: fetches the `.deb`, its signature and its checksum file into a folder in the app data directory, with progress and cancel.
- Verification: checks the signature with the embedded public key, then the SHA-256 of the `.deb`. Before the install step, the privileged helper copies the verified package into its root-owned staging folder, and it re-checks the staged file, so the file cannot change between verification and install.
- Install: requests privilege through polkit, runs `apt-get install` on the staged file and maps apt's exit status to a typed outcome. Success is reported only when the installed version has changed.
- Restart: after a successful install, the app restarts into the new version. The machine service reconciles through [CW-H04](CW-H04.md) without replaying prompts.
- Rollback: keeps the previous `.deb` as described in open decision 5 and offers "Reinstall previous version" when the new version fails its start check.
- Settings › Updates page with the states listed in [SCREENS](../SCREENS.md#application-updates-desktop), including the rule that an install does not start while a worker runs on the machine.
- Feed fixtures for tests: newer, same, older, missing asset, missing signature, bad signature, bad checksum and truncated download.

## Acceptance

Positive:

- A newer signed release on the feed is shown with its version and release notes. When no newer release exists, the page shows "Up to date".
- After owner confirmation and privilege approval, the verified package installs, the app restarts into the new version and the installed version is shown.
- When the new version fails its start check, "Reinstall previous version" restores the previous version.

Negative:

- A release with a missing, invalid or mismatched signature or checksum is never installed. The state reads "Verification failed · nothing installed".
- A package that differs from the verified file at install time is not installed.
- The privileged helper refuses any path outside its root-owned staging folder.
- No privilege prompt appears before verification has passed.
- An older or equal version is not offered as an update. Rollback uses its own explicit action.
- An install does not start while a worker runs on the machine. The update does not stop a worker by itself, and no prompt is replayed.
- An unreachable GitHub feed shows "Check failed" with the time of the last check, and nothing is installed.
- Release notes are rendered as plain text. No HTML or script runs.
- Feed tokens and other credentials are neither logged nor displayed.
- A failed apt install leaves the installed version unchanged and says so.

## Dependencies

- [CW-F01](CW-F01.md): the `.deb` package and the pinned toolchain.
- [CW-H01](CW-H01.md): one machine-service instance, so an update never leaves two services running.
- Gate records: CW-D05 (Gate D) must be recorded before implementation.

## Validation

- Unit tests against the feed fixtures: version comparison, same and older versions, missing asset, signature and checksum failure, truncated download. Each failure fixture must show that no install call was made.
- Helper tests: rejected path outside the staging folder, staged file changed after verification, and apt exit codes mapped to typed outcomes. Use a throwaway test key, never the release key.
- Clean Ubuntu update run in [CW-Q04a](CW-Q04a.md): install version N, serve a signed fixture N+1, update through the app, then confirm the installed version, the service health and that no prompt was replayed. Not run until CW-F01 produces a `.deb`.
- Rendered run of Settings › Updates in the Ubuntu Tauri 2 shell at 1280×800 and 1920×1080, covering each state. Not run until CW-F01 provides the shell.
- Record exact commands, environment and outcomes. A check that was not run is recorded as not run.

## Documentation impact

- [SCREENS](../SCREENS.md): the Application updates section (added with this item).
- [Platform policy](../platforms.md) and [roadmap](../roadmap.md): the distribution method once open decision 1 is recorded.
- [Risks and open decisions](../risks-and-open-decisions.md): update the release-distribution entry only when the owner records the decisions.
- [Security design](../security.md): signing-key handling and the privileged helper boundary, once implemented.

## Size

L: release feed, verified download, privileged helper and state model. If effort exceeds L, split before implementation into (1) release detection and the Updates page, (2) verified download and privileged install, and (3) failure and rollback states.
