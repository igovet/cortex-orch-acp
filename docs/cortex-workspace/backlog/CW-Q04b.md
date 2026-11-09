# CW-Q04b: Android APK upgrade checks

**Status:** planned · **Phase:** 3+ · **Size:** M

Split from [CW-Q04](CW-Q04.md) for the Android platform.

## Goal
On the owner's physical Android device, verify that installing a newer APK over an older one preserves pairing, the Keystore-wrapped device key and the event cursor. Document the downgrade and rollback behavior.

## Requirements
CW-R08 (secure enrollment and reconnect without extra installed software), CW-R12 (Android qualification) and CW-R16 (protect device identities). See [requirements](../requirements.md).

## Inputs
- CW-N02 (Keystore-wrapped key) and CW-N03 (pairing).
- [Platform policy](../platforms.md): Android requires physical-device qualification.
- CW-Q03b (physical device, which this item reuses).

## Deliverables
- A two-version upgrade run: previous APK built from the release branch, then the new APK, on the physical device.
- A record of the NodeId, pairing state, cursor, client cache and FCM token state before and after the upgrade. This item also records the Android client-cache state that CW-Q05 (desktop-only) does not cover.
- A documented downgrade and rollback procedure, including Android's version-code rule.

## Acceptance
**Positive**
- After the upgrade, the NodeId and pairing are unchanged. The cursor is preserved, and the app resumes from it.
- The FCM token is retained or re-registered, and the desktop record matches.

**Negative**
- An install with a lower version code is refused by Android. The procedure documents this.
- An upgrade signed with a different key is refused by Android. The refusal is recorded.
- If the Keystore key is lost during upgrade, the app shows an explicit re-enrollment state. It does not generate a new identity silently.
- Reject stale, unauthorized, missing or conflicting input: a missing before-state record means the upgrade is not verified.

## Dependencies
CW-Q03b. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively.

## Validation
- Physical-device upgrade run with `adb install -r` on the two builds. Record the build identifiers and exit codes.
- Emulator runs are not qualifying evidence.
- No check has been run for this planned item.

## Documentation impact
- [Platform policy](../platforms.md): Android upgrade and downgrade rules.
- [Remote access](../remote-access.md): key-loss recovery through re-enrollment.
