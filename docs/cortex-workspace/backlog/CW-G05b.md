# CW-G05b: Android Git screens

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Build the React Native Git screens for Android: status, read-only diff, stage and unstage, commit confirmation, branch switch and push confirmation. They use the same service commands and typed outcomes as the desktop panel in CW-G05a. Mobile confirmations show the full remote and branch names. No Android path turns an acceptance into a commit or a push.

## Requirements
- CW-R11: Git status, stage, commit, branch and push are available on mobile with explicit conflicts.
- CW-R05: acceptance stays separate from Git actions.

See [requirements](../requirements.md).

## Inputs
- [CW-G04](CW-G04.md) (confirmed push) and [CW-A02](CW-A02.md) (Android app shell and sync).
- [Screens](../SCREENS.md) (Android Git is accessible but secondary).
- [ADR 0007](../adr/0007-result-acceptance.md) and [ADR 0009](../adr/0009-push.md).

## Deliverables
- Status, read-only diff, stage and unstage, commit confirmation, branch switch and push confirmation screens.
- Confirmation screens show the full remote URL and branch name, wrapped or expanded, never truncated silently.
- Offline state disables mutating actions and shows the reason.

## Acceptance
Positive:
- On a physical Android device, a stage and commit round trip against the fixture repository shows the same typed outcomes as the desktop panel.

Negative:
- No accept-to-commit or accept-to-push path exists.
- The push confirmation is disabled while offline.
- An unknown push outcome is not retried when the app resumes.
- At 360×800 the remote and branch names are fully readable, or expanded on request.

## Dependencies
- [CW-G04](CW-G04.md), [CW-A02](CW-A02.md).
- Gate records: CW-D05 (Gate D) and CW-M03 (Gate M).

## Validation
- Physical Android device: scripted round trip and an offline attempt.
- Screenshots at 360×800 and 412×915.
- Record device, Android version, exact commands and outcomes.

## Documentation impact
- [Screens](../SCREENS.md) and [platforms](../platforms.md) once measured on the device.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
M: the screens reuse the typed outcomes of CW-G05a and are bounded by the mobile confirmation rules.
