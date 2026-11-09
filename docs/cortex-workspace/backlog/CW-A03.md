# CW-A03: Android result screens and file-edit WebView host

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Build the native React Native result screen (evidence, revision digest and an entry to the acceptance decision) and a native changed-files list with read-only hunks. Also build the file-edit screen, which hosts the CodeMirror 6 bundle from [CW-I05](CW-I05.md) inside react-native-webview. React Native owns navigation: the back gesture, the stack and the header.

## Requirements
- CW-R03: acceptance is a decision kind, separate from runtime completion and Git.
- CW-R05: evidence, missing checks and the exact revision are shown.
- CW-R10: bounded, path-checked file editing with protection against concurrent human and agent changes.

See [requirements](../requirements.md).

## Inputs
- [Screens](../SCREENS.md): Android result and diff, and file editing (secondary screens).
- [IDE and Git](../ide-and-git.md): touch editing; the WebView bridge is a candidate that is subject to feasibility and input tests.
- The bridge contract in CW-I05, the acceptance contract in CW-C07, and [ADR 0007](../adr/0007-result-acceptance.md).

## Deliverables
- Native result screen: evidence list, revision digest, and an entry to the acceptance decision in [CW-A02](CW-A02.md)'s decision flow.
- Native changed-files list and read-only hunk viewer. The diff is not rendered by CodeMirror on Android.
- File-edit screen: WebView hosting the CodeMirror bundle, a native header with dirty and conflict states, saves through the CW-I05 bridge, and a compare, reload or save-as choice on conflict.

## Acceptance
Positive:
- An edit followed by a save round-trips through the hash-checked contract from CW-I03 and CW-I05 on the physical device.
- A save against a stale base hash opens the native compare, reload and save-as choice.

Negative:
- The WebView never intercepts the Android back gesture. Going back from a dirty editor shows a discard prompt; it never discards silently.
- The accept action is absent when the digest is stale.
- An unrun or unknown check is never shown as passed.
- With the soft keyboard open, the caret line stays visible above the keyboard on the physical device.

## Dependencies
- [CW-A02](CW-A02.md) (Android decision and sync), CW-C07 (version-bound acceptance), [CW-I05](CW-I05.md) (bridge contract).
- Gate records: CW-D05 (Gate D) and CW-M03 (Gate M).

## Validation
- Physical Android device: scripted back-gesture navigation with a dirty buffer; save round trip; stale-hash conflict fixture.
- Screenshots of the result screen and the editor at 360×800 and 412×915, with the keyboard open.
- Emulator runs are not reported as device qualification.
- Record exact commands, device and outcomes.

## Documentation impact
- [Screens](../SCREENS.md): Android result and file-edit states.
- [IDE and Git](../ide-and-git.md) and [roadmap](../roadmap.md) when the bridge decision is qualified.

## Size
L: two native screens and an embedded editor with navigation and keyboard constraints. The bridge contract itself stays in CW-I05.
