# CW-S09: Real Tauri shell with a closed native bridge

**Status:** built; native build and journey pass in the container build · **Phase:** 3+ · **Size:** L

## Goal

Create the production Tauri 2 window for Ubuntu. It loads only the bundled local interface and reaches the service only through a closed allowlist of operations in the native bridge. No arbitrary path, launch or RPC is exposed to the web content.

## Requirements

CW-R12 (Ubuntu first) and CW-R13 (the accepted desktop direction). See [requirements](../requirements.md).

## Inputs

- [CW-F01](CW-F01.md) and [CW-S01](CW-S01.md): pinned toolchains. Rust 1.99.1 and `@tauri-apps/cli` 2.10.1 were observed on 2026-11-05.
- [platforms](../platforms.md): the WebKitGTK 4.1 development packages must be installed by the owner before a native build. Until then this item is blocked.
- [CW-S03](CW-S03.md) and [CW-S08](CW-S08.md).

## Deliverables

- `apps/cortex-workspace-desktop/` with `src-tauri`, a capability allowlist for the bridge, navigation and content-security policy that refuses external navigation, and exact Tauri crate pins instead of prototype version ranges.
- A native bridge client to the service socket that frames requests and never exposes the socket path or credentials to JavaScript.

## Acceptance

Positive:

- Only the bundled local window can invoke the allowed operations.
- Closing the window leaves the service child running.
- The rendered window's observations are recorded as structured outcomes and text, not as retained images.

Negative:

- External navigation is refused.
- An unlisted operation, or any argument naming a path or a launch command, is refused.
- Chromium or headless browser results are not reported as WebKitGTK results.

## Dependencies

[CW-S03](CW-S03.md) and [CW-S08](CW-S08.md).

## Validation

- TypeScript checks first, then the locked Cargo and Tauri check and build. Blocked until the WebKitGTK development packages are installed; record the blocker, not a pass.
- Rendered Ubuntu observation of the window, with the exact WebKitGTK and build versions recorded.

## Documentation impact

- [platforms](../platforms.md), [architecture](../architecture.md), [security](../security.md) and [ADR 0003](../adr/0003-tauri-stack.md), where the bridge boundary is documented.
