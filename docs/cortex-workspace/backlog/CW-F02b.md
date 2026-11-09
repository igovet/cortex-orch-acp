# CW-F02b: React Native (New Architecture) minimal spike on the owner's physical device: FCM push, Keystore round-trip and jank capture

**Status:** planned · **Phase:** 3+ · **Size:** L

Split from [CW-F02](CW-F02.md): the combined Tauri and React Native spike exceeds the two-day limit for size L, so each stack has its own time-boxed item.

## Goal

Build a minimal React Native app with the New Architecture (Fabric and TurboModules) enabled and one list screen. On the owner's physical phone, measure the same four checks as [CW-F02a](CW-F02a.md) for the React Native option: one real FCM push delivered to the app, one Android Keystore-backed secret written and read back, jank during a fixed scroll script, and cold start. The spike is time-boxed to two days. Checks not finished inside the box are reported as not run.

## Requirements

CW-R08, CW-R09, CW-R12, CW-R14 and CW-R16. See [requirements](../requirements.md).

## Inputs

- [remote-access](../remote-access.md) and [security](../security.md): push and credential requirements.
- [performance](../performance.md): Android Now cold start and jank budgets.
- [ADR 0003](../adr/0003-tauri-stack.md).
- Candidate libraries are recorded by exact version: a React Native Firebase messaging integration for FCM, and a Keystore-backed keychain library (for example `react-native-keychain`). Record each library's status at spike time.
- The New Architecture must be active in the built app. Confirm it at runtime (for example through the React Native version and renderer report); otherwise the run does not test the stack under evaluation.

## Deliverables

- Minimal React Native app with exact versions, build command and output hash, and the confirmed New Architecture record.
- FCM record: Firebase project identity (no credentials), token handling, one delivered message, and a delivery record with timestamp and device log excerpt.
- Keystore record: one secret written and read back, showing storage through the Keystore-backed facility.
- Jank record: `adb shell dumpsys gfxinfo <package>` before and after a fixed scroll script, with the janky-frame percentage.
- Cold start record: `adb shell am start -W <package>/<activity>` TotalTime over repeated runs.
- Result record with device model, Android version, build fingerprint, and what was not run.

## Acceptance

Positive:

- One real FCM push reaches the physical device, and the app records receipt.
- One Keystore-backed secret round-trips; the value read back equals the value written.
- Jank and cold-start figures are recorded with method and device and compared with [performance](../performance.md) targets.

Negative:

- A build where the New Architecture is not active does not count as this spike.
- A message sent from a console or emulator, or a notification shown without app receipt, does not satisfy the push check.
- A secret kept in AsyncStorage or any plain storage fails the Keystore check.
- With notification permission denied or push services unavailable, the app shows an explicit "push unavailable" state. A silent failure fails this check.
- Exceeding the two-day time box stops the spike. Partial results are recorded as incomplete, never as a pass.

## Dependencies

CW-M03 (Gate M).

## Validation

- `adb devices`, device model and Android version output recorded.
- `new-architecture-active`: runtime confirmation output.
- `fcm-delivery`, `keystore-roundtrip`, `gfxinfo-jank` and `cold-start-am`: the same records as CW-F02a, for the React Native build.
- Run on the owner's physical device only. Emulator output is informational.

## Documentation impact

- Decision record in [CW-F02](CW-F02.md); the React Native row in [platforms](../platforms.md); push and Keystore entries in [risks](../risks-and-open-decisions.md).
