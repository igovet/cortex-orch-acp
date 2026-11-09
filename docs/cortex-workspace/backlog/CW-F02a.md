# CW-F02a: Tauri 2 Android minimal spike on the owner's physical device: FCM push, Keystore round-trip and jank capture

**Status:** planned · **Phase:** 3+ · **Size:** L

Split from [CW-F02](CW-F02.md): the combined Tauri and React Native spike exceeds the two-day limit for size L, so each stack has its own time-boxed item.

## Goal

Build a minimal Tauri 2 Android app with one list screen. On the owner's physical phone, measure the four checks that CW-F02 needs for the Tauri option: one real FCM push delivered to the app, one Android Keystore-backed secret written and read back, jank during a fixed scroll script, and cold start. The spike is time-boxed to two days. Use the pinned toolchain from [CW-F01](CW-F01.md). Checks not finished inside the box are reported as not run.

## Requirements

CW-R08, CW-R09, CW-R12, CW-R14 and CW-R16. See [requirements](../requirements.md).

## Inputs

- [remote-access](../remote-access.md) and [security](../security.md): push and credential requirements.
- [performance](../performance.md): Android Now cold start and jank budgets.
- [ADR 0003](../adr/0003-tauri-stack.md).
- Candidate plugins are recorded by exact version: a Keystore plugin, a community FCM push plugin, and the official notification plugin (which provides local notifications only). Record each plugin's maintenance and release status at spike time. A pre-release or community status is recorded as a risk. It is not a blocker or a pass by itself.

## Deliverables

- Minimal Tauri 2 Android project with exact versions, build command and output hash.
- FCM record: the Firebase project identity (no credentials), token handling, one delivered message, and a delivery record with timestamp and a device log excerpt.
- Keystore record: one secret written and read back, showing that storage went through the Keystore-backed facility and not a preferences file.
- Jank record: `adb shell dumpsys gfxinfo <package>` before and after a fixed scroll script, with the janky-frame percentage.
- Cold start record: `adb shell am start -W <package>/<activity>` TotalTime over repeated runs.
- Result record with device model, Android version, build fingerprint, and what was not run.

## Acceptance

Positive:

- One real FCM push reaches the physical device, and the app records receipt.
- One Keystore-backed secret round-trips; the value read back equals the value written.
- Jank and cold-start figures are recorded with method and device and compared with [performance](../performance.md) targets.

Negative:

- A message sent from a console or emulator, or a notification that appears in the tray without the app receiving it, does not satisfy the push check.
- A secret kept in plain app storage (for example preferences or AsyncStorage) fails the Keystore check.
- With notification permission denied or push services unavailable, the app shows an explicit "push unavailable" state. A silent failure fails this check.
- Exceeding the two-day time box stops the spike. Partial results are recorded as incomplete, never as a pass.

## Dependencies

CW-M03 (Gate M) and CW-F01.

## Validation

- `adb devices`, device model and Android version output recorded.
- `fcm-delivery`: delivery record as described above.
- `keystore-roundtrip`: write, read and compare output.
- `gfxinfo-jank` and `cold-start-am`: exact command output.
- Run on the owner's physical device only. Emulator output is informational and does not count as a check result.

## Documentation impact

- Decision record in [CW-F02](CW-F02.md); the Tauri Android row in [platforms](../platforms.md); push and Keystore entries in [risks](../risks-and-open-decisions.md).
