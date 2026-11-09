# CW-I05: Android small-file edit contract for the CodeMirror 6 WebView bridge

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Define and implement the bridge contract between React Native and a WebView-hosted CodeMirror 6 bundle, for small files. The contract covers load, save with hash check, conflict result, undo and redo, and dirty state. The bundle and its configuration are built from the same source as the desktop editor in CW-I04a. The small-file size bound is proposed at 256 KiB and is confirmed by measurement.

## Requirements
- CW-R10: bounded, path-checked file editing with concurrent-change protection on mobile.
- CW-R16: unauthorized, stale and unknown messages are rejected.

See [requirements](../requirements.md).

## Inputs
- [IDE and Git](../ide-and-git.md): Android uses a touch-oriented editing workflow; the WebView bridge is a candidate subject to feasibility and input tests. Do not assume Monaco mobile support.
- The save contract in [CW-I03](CW-I03.md).
- [ADR 0003](../adr/0003-tauri-stack.md) and the Android stack bake-off in [CW-F02](CW-F02.md).
- The app shell and sync in [CW-A02](CW-A02.md).

## Deliverables
- Versioned bridge message schema with message identifiers. Unknown versions are rejected.
- Load (content and base hash), save (base hash check), conflict result, and undo history retained in WebView state.
- Shared bundle build configuration with the desktop editor.
- Size guard with an explicit "too large for mobile edit" state.

## Acceptance
Positive:
- A save through the bridge produces the same typed results as desktop (saved, conflict) for the same fixture cases.
- Undo history survives an app background and foreground cycle on the physical device.

Negative:
- A file above the size bound is refused with the explicit state. It is never truncated.
- Bridge messages with an unknown version, an unsolicited message identifier, or a document that is not loaded are ignored and logged without content.
- A WebView reload never silently discards unsaved content. The content is restored from the React Native-side buffer, or the user is asked.

## Dependencies
- [CW-A02](CW-A02.md), [CW-I03](CW-I03.md), [CW-F02](CW-F02.md).
- Gate records: CW-D05 (Gate D) and CW-M03 (Gate M).

## Validation
- Automated bridge contract tests in the React Native test runner. These do not require a device.
- Physical-device test: edit, background, foreground, undo; record device and Android version.
- Save-conflict fixture through the bridge.
- Bundle size measured and recorded.

## Documentation impact
- [IDE and Git](../ide-and-git.md) for the mobile editing contract once qualified.
- [Platforms](../platforms.md) and [roadmap](../roadmap.md) when qualification changes.

## Size
L: a versioned bridge contract, WebView lifecycle handling, and a physical-device undo test.
