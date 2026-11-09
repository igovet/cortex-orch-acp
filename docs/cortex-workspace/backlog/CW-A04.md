# CW-A04: Android credential storage and measured background connection

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Store the device enrollment credential in Android Keystore-backed storage through react-native-keychain. Make revocation from the desktop invalidate the stored credential on the next contact. Measure the operating-system limits on background connections on the owner's device and document the measured values. Do not promise keep-alive behaviour that has not been measured.

## Requirements
- CW-R08: enrollment survives restart; revocation takes effect.
- CW-R09: push timeliness depends on background connection limits, which must be measured and stated honestly.
- CW-R16: credentials and device identities are protected; stale or revoked devices are rejected.

See [requirements](../requirements.md).

## Inputs
- [Security](../security.md): secrets in OS-protected storage; revoke immediately; never log tokens, source, prompts or private keys.
- CW-N03 (enrollment) and CW-N04 (revocation enforced at the accept step).
- [ADR 0009](../adr/0009-push.md) and [risks](../risks-and-open-decisions.md): no delivery SLA or no-GMS support is assumed.
- [Platforms](../platforms.md): physical-device qualification of lifecycle and networks.

## Deliverables
- Credential store module with read, write and clear operations. It never logs the credential.
- Backup exclusion in the Android manifest.
- Revocation handler: clears the credential on a revoked response and stops reconnection attempts.
- Measurement record: device model, Android version, battery-optimization setting, Doze observations from `adb shell dumpsys deviceidle`, and measured reconnection timing. Documented limits apply to that device only.

## Acceptance
Positive:
- The credential survives an app restart on the physical device.
- After revocation, the next connection attempt fails and the stored credential is cleared. Reconnection attempts stop.
- The keep-alive limits in the documentation come from the recorded measurements and are labelled as measured on that device.

Negative:
- The stored credential is not readable as plaintext from app data copied off a debuggable build (run-as or backup extraction). The key is non-exportable.
- No token or credential appears in captured logcat output during enrollment, reconnection or revocation.
- No keep-alive or push-timeliness promise is written into the documentation without a measurement.

## Dependencies
- [CW-A01](CW-A01.md) (app shell), CW-N03 (enrollment), CW-N04 (device allow-list and immediate revocation).
- Gate records: CW-D05 (Gate D) and CW-M03 (Gate M).

## Validation
- Physical device: restart test; `adb logcat` capture grepped for the credential pattern during enrollment, reconnection and revocation.
- Run-as inspection of app data on a debuggable build to confirm no plaintext credential.
- `adb shell dumpsys deviceidle` and a timed reconnection test against a revoked-device fixture from CW-N04.
- Record commands, device and outcomes. Unmeasured limits are reported as unverified.

## Documentation impact
- [Security](../security.md): storage and revocation behaviour once implemented.
- [Platforms](../platforms.md): measured background limits per device.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
M: one secure store, one revocation path and one measurement protocol on a physical device.
