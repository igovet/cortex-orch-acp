# CW-F01: Pin toolchain versions and stand up the real Ubuntu Tauri 2 + SolidJS + TypeScript shell

**Status:** planned; a narrow subset is built by CW-S01 and CW-S09 and is not closed by them · **Phase:** 3+ · **Size:** M

## Goal

Pin every desktop toolchain input (Rust toolchain, Tauri 2 CLI and crates, Node or the chosen package manager, SolidJS, TypeScript, bundler) to exact versions recorded in committed lock files. Then build a minimal Tauri 2 + SolidJS + TypeScript shell that runs on the reference Ubuntu LTS and renders a "hello" screen through WebKitGTK. Electron is a rejected alternative ([ADR 0003](../adr/0003-tauri-stack.md)) and is not used. This repository has no desktop shell directory to migrate, so this item creates the first one. The shell calls no Cortex runtime and reads no project data.

## Requirements

CW-R12 (Ubuntu first; other platforms need separate qualification) and CW-R14 (measured budgets, not claims). See [requirements](../requirements.md).

## Inputs

- [architecture](../architecture.md): the desktop client is a view; the Rust machine service owns resources. Only the shell is in scope here.
- [platforms](../platforms.md): Ubuntu is the first qualified desktop platform; WebKitGTK GPU and session behavior must be measured, not assumed.
- [performance](../performance.md): cold board target below 700 ms and idle memory below 150 MB per window. Record method, hardware and build for each measurement.
- [ADR 0003](../adr/0003-tauri-stack.md): Tauri 2 with SolidJS on desktop.
- [desktop approval](../design/desktop-approval.md): the shell is built after the Gate D record (CW-D05), which exists. Gate M (mobile) does not gate it.
- Toolchain observations (2026-11-05, owner-approved user-level Rust install; not a build): `rustc 1.99.1`, `cargo 1.99.1` and `rustup 1.29.3` under the home directory (`~/.cargo/bin`); Ubuntu 26.04.1 LTS observed; the Tauri CLI pin in `prototypes/cortex-workspace/desktop/package.json` is `@tauri-apps/cli` 2.10.1. Native builds run in a Docker container, and the owner installs no development packages on the host (owner decision of 2026-11-05). The container installs `libwebkit2gtk-4.1-dev`, `libssl-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev` and `libxdo-dev`. Prerequisites are in [platforms](../platforms.md).

## Deliverables

- Pinned toolchain manifest and lock files, plus a check that fails on any unpinned or drifted version.
- Dependency and license inventory generated from the lock files and committed as a text table.
- Minimal shell with one Solid "hello" route, no network access and no Cortex calls.
- `tauri dev` and `tauri build` commands documented for the reference Ubuntu LTS.
- Package format and version source recorded: the `tauri build` output for Ubuntu is to be a `.deb` (owner decision of 2026-11-05; the produced format is verified here), and the package version source is named (proposed: the GitHub release tag). [CW-U16](CW-U16.md) depends on both.
- Baseline record: Ubuntu release, kernel, webkit2gtk version, GPU vendor, driver version, X11 or Wayland session, and the command or observation that produced each value. A value that cannot be detected is recorded as `unknown`.

## Acceptance

Positive:

- `tauri build` completes on the reference Ubuntu LTS and produces an installable package; `tauri dev` opens a window.
- The hello screen is observed rendering in WebKitGTK on the reference machine. A headless DOM dump alone does not satisfy this.
- The baseline record names GPU vendor, driver and session type from observed output.

Negative:

- A missing or unpinned toolchain version fails the pin check. A lock-file change without a matching manifest update fails it too.
- Navigation to a non-local URL is refused by the shell (denied-navigation check).
- An unsupported Ubuntu release or a missing webkit2gtk makes the build report "unsupported"; no success is recorded.
- A dependency whose license is not on the agreed list appears in the inventory flagged for owner review; it is not silently accepted.

Unrun or unsupported cases are recorded as such. A CLI-only run is not qualification of the rendered shell.

## Dependencies

CW-D05 (Gate D). Gate M does not gate this item; the Gate D record exists.

## Validation

- `pin-check`: compares the manifest with the lock files; expected exit 0 on the committed tree.
- `ubuntu-build`: `tauri build` on the reference Ubuntu LTS; record exit code and artifact hash.
- `ubuntu-hello-render`: rendered observation in WebKitGTK; retain the screenshot outside the repository.
- `navigation-denied`: attempt navigation to an external URL; record the refusal.
- `baseline-record`: GPU, driver, session and webkit2gtk output with the exact commands used.

## Documentation impact

- [platforms](../platforms.md) and [roadmap](../roadmap.md): record the qualified Ubuntu release, shell version and baseline.
- [risks](../risks-and-open-decisions.md): update the WebKitGTK entry with the observed baseline.
- No README or feature page changes until a shell ships.
