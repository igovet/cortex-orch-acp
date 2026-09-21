# Historical, rejected host-repair proposal for production Zed sandbox refusal

Status: **rejected and not part of the ACP setup**. The proposal is retained
only to preserve the investigation record. Cortex must work without installing
this profile or changing Linux/AppArmor policy.

The production Zed/Cortex process reported
`sandbox_userns_denied`; matching kernel audit records showed AppArmor denying
`setpcap` and a write to `uid_map`. A sandbox probe in the disposable Zed
profile on virtual display `:92` passed, so it cannot qualify production.

Host facts observed on 2026-09-19:

- Ubuntu 24.04.5 LTS; AppArmor parser 4.0.1; ABI `4.0` present.
- `bubblewrap` 0.9.0 at `/usr/bin/bwrap`; not setuid.
- `kernel.unprivileged_userns_clone=1` and
  `kernel.apparmor_restrict_unprivileged_userns=1`.
- No `/etc/apparmor.d/bwrap-userns-restrict` file is installed.

Candidate: the upstream AppArmor **ABI 4.0**
[`bwrap-userns-restrict` profile at commit `1979af7710d0`](https://gitlab.com/apparmor/apparmor/-/raw/1979af7710d0/profiles/apparmor/profiles/extras/bwrap-userns-restrict).
The downloaded candidate is held outside the repository at
`/tmp/cortex-acp-live-ypkwriti/final/bwrap-userns-restrict.1979af7710d0`, with
SHA-256 `634d3d3427c483f123cb5ed53b71ea13040187e07d9f67ca74421d42a6170f0e`.
Local `apparmor_parser -Q -T -K` accepted it without loading it into the
kernel and resolved the `bwrap` and `unpriv_bwrap` profile names.

This profile grants `bwrap` the namespace setup operations and stacks a
capability-denying profile on its children. It applies to `/usr/bin/bwrap`
system-wide, so it can affect other programs using bubblewrap. Ubuntu's
[AppArmor guidance](https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007)
specifically recommends administrator review for such a profile. The current
ACP path does not require this profile: it runs native workers directly in
the exact Zed project, with no claim of filesystem isolation.

The installation procedure below is historical only and must not be used for
the current ACP product. If an administrator had approved installation, they
would first recheck the candidate hash
and any existing loaded `bwrap` profile, retain a backup if a file appears,
then install the exact candidate as `/etc/apparmor.d/bwrap-userns-restrict`
and load it with `apparmor_parser`. Test the fixed `bwrap` probe from a fresh
production Zed/Cortex process using `/cortex-status` before any model call;
the old path required it to show `sandbox: ok`. If the probe or another
bubblewrap user regresses, remove the loaded profile and restore the previous
file. A passing virtual Zed probe or parser-only validation is insufficient.
