# CW-N02: Integrate iroh Android bindings through the selected mobile stack; store device key with Android Keystore and validate the bridge

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Run the phone-side iroh endpoint through the official iroh-ffi Kotlin bindings inside the selected Android stack. The device NodeId key is wrapped at rest by Android Keystore (StrongBox where the device supports it).

## Requirements
CW-R08 (secure enrollment and reconnect without extra installed software) and CW-R16 (protect device identities). See [requirements](../requirements.md).

## Inputs
- [Remote access](../remote-access.md) and [security design](../security.md).
- CW-F02 outcome: the go/no-go decision between React Native (New Architecture) and Tauri Android. The provisional choice is in [risks and open decisions](../risks-and-open-decisions.md).
- The official `n0-computer/iroh-ffi` Kotlin bindings. Upstream classifies them as tier 2 support, so the pinned version and any patches are recorded.
- A physical Android device owned by the project owner.

## Deliverables
- A bridge module that exposes endpoint create, dial and close to the app. It is built from the pinned iroh-ffi Kotlin bindings.
- Device key handling. The bridge generates or loads the key and stores key material encrypted by an AES key held in Android Keystore, using StrongBox when the device reports it and TEE otherwise. The chosen backing is recorded per device. This is key wrapping, not hardware generation of the Ed25519 key.
- A secure-storage fixture covering wrap, unwrap, app-data clearing and the key-absent state.
- Out of scope: pairing (CW-N03), push (CW-P02) and content transfer.

## Acceptance
**Positive**
- An endpoint is created on the physical device. Its NodeId is stable across app restart and device reboot.
- The bridge reports whether StrongBox or TEE backing was used.
- The stored key blob is not readable from an `adb backup` export. Backup exclusion is configured for the key store. If the backup exposes it, the item fails.

**Negative**
- If Keystore key invalidation occurs (for example, a lock-screen credential change), the bridge reports `key_unavailable` and does not silently generate a new identity.
- No plaintext private key exists in app storage. Verified on the device with a debug build and file inspection.
- Logs and diagnostics contain no key material.
- Reject revoked or expired credentials: not applicable to this item. Device revocation is covered by CW-N04.

## Dependencies
CW-F02. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively through CW-F02 → CW-M03.

## Validation
- Unit tests for the bridge wrapper and secure-storage fixture under the Android Gradle build (`./gradlew testDebugUnitTest`).
- Instrumented tests on the physical device (`./gradlew connectedDebugAndroidTest`), with `adb` inspection of app storage and backup behavior.
- An emulator run is not physical-device evidence and does not satisfy this item.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): phone-side key storage.
- [Security design](../security.md): Keystore wrapping, backup exclusion and key-invalidation behavior.
- [Platform policy](../platforms.md): Android physical qualification remains pending until CW-Q03b.
