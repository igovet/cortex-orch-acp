# CW-S01: Local slice prerequisites: Rust toolchain pin, Cargo workspace skeleton and document reconciliation

**Status:** built; no-paid checks pass · **Phase:** 3+ · **Size:** S

## Goal

Make the local desktop slice buildable in principle without building product behavior. Pin the Rust toolchain and the Cargo workspace inputs, record the host prerequisites that were observed, and reconcile the Workspace documents with the owner decisions of 2026-11-05: mobile is deferred, Gate M does not gate desktop work, and the local vertical slice comes before the desktop screens and before remote machines, relay and push.

## Requirements

CW-R12 (Ubuntu first; other platforms need separate qualification) and CW-R14 (measured budgets, not claims). See [requirements](../requirements.md).

## Inputs

- [platforms](../platforms.md): the observed toolchain versions and the container build route, which needs no host development packages.
- [CW-F01](CW-F01.md): the pinned shell toolchain. This item provides its Rust part.
- [roadmap](../roadmap.md): build order and milestone M2.
- The Tauri CLI pin in `prototypes/cortex-workspace/desktop/package.json` (`@tauri-apps/cli` 2.10.1). Prototype dependency ranges such as `version = "2"` are not acceptable production pins.

## Deliverables

- `rust-toolchain.toml` with an exact stable version, and a root `Cargo.toml` with a committed `Cargo.lock`. Default workspace members exclude the Tauri application, so service and protocol tests need no GTK or WebKitGTK.
- A pin check that fails on a floating toolchain channel, an unpinned crate range, or drift between the toolchain file, the lock file and the CLI pin.
- A host prerequisite probe that records, for each item, present or missing with the observed command and exit code: Node, npm, Python test dependencies and the user service manager. It prints no environment values and no process command lines.
- Document reconciliation for the Workspace index, roadmap, backlog index, platforms and the Gate D and Gate M records, as listed under documentation impact.

## Acceptance

Positive:

- The pin check exits 0 on the committed tree, and the exact Rust version is recorded.
- The prerequisite probe records every item as present or missing, with no inferred absence.

Negative:

- A floating toolchain such as `stable` without a version, or a crate range in the Tauri application manifest, fails the pin check.
- A native Tauri build is not reported as passed while the WebKitGTK development packages are missing. It is reported as blocked.

## Dependencies

[CW-D05](CW-D05.md) (Gate D record, passed). Gate M ([CW-M03](CW-M03.md)) is not a dependency.

## Validation

- `pin-check`: compares the toolchain file, lock file and CLI pin; expected exit 0.
- `prerequisite-probe`: records present or missing per item with the exit code of each probe.
- `cargo metadata --locked` at the workspace root; it must not require GTK.

## Documentation impact

- [platforms](../platforms.md), [roadmap](../roadmap.md), [backlog index](index.md), [CW-F01](CW-F01.md) and [desktop approval](../design/desktop-approval.md).
