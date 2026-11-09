# CW-F03: CodeMirror 6 (+@codemirror/merge, +@codemirror/lsp-client) and xterm.js (@xterm/addon-webgl + canvas fallback) fixture inside the real Tauri webview

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Build a fixture page inside the real Tauri 2 webview on Ubuntu. It hosts a CodeMirror 6 editor with a 5,000-line file, `@codemirror/merge` for a unified and split diff, and `@codemirror/lsp-client` wired but without a language server attached. It also hosts an xterm.js terminal on `@xterm/addon-webgl` with `@xterm/addon-canvas` as fallback, attached to a throwaway local PTY. The PTY is fixture-only; production PTY ownership belongs to CW-T01. Monaco and Electron are rejected alternatives ([ADR 0003](../adr/0003-tauri-stack.md)); this fixture does not use them.

## Requirements

CW-R10 (bounded editor, search, terminal and LSP), CW-R12 (Ubuntu qualification) and CW-R14 (measured budgets). See [requirements](../requirements.md).

## Inputs

- [ide-and-git](../ide-and-git.md): CodeMirror 6 for editing and diff; xterm.js for service-owned PTYs with graphics fallback; Monaco is not a mobile base.
- [performance](../performance.md): 16.7 ms frame time during scroll and drag; no visible terminal stutter; safe fallback on graphics-context loss.
- [ADR 0003](../adr/0003-tauri-stack.md): CodeMirror 6 replaces Monaco.
- [CW-F01](CW-F01.md): the shell and pinned toolchain.

## Deliverables

- Fixture page with exact package versions for CodeMirror 6, `@codemirror/merge`, `@codemirror/lsp-client`, xterm.js and both renderer addons.
- Frame-timing method recorded in the result, for example the webview inspector's performance capture, with hardware and build.
- A control that forces WebGL context loss through the addon's context-loss path (for example by calling the browser `WEBGL_lose_context` extension on the terminal canvas) so the fallback can be observed.
- Note on which `@codemirror/merge` view (unified or split) is validated for CW-U04.

## Acceptance

Positive:

- The 5,000-line file scrolls at sustained frame time of 16.7 ms or less, measured with the recorded method on the reference hardware.
- The PTY echo test shows no visible stutter, confirmed by rendered observation as well as timing.
- A forced WebGL context loss switches the terminal to the canvas renderer without a crash, and the terminal keeps accepting input.

Negative:

- A frame-time figure without a recorded method and hardware is rejected.
- Measurements taken in a browser or in a non-Tauri webview do not count for this item.
- If the fallback path is not exercised, the item cannot pass, even if the normal renderer is fast.
- A PTY throughput run that drops output or hangs is recorded as a failure, not as a slow pass.

## Dependencies

CW-F01.

## Validation

- `cm6-5000-line-scroll`: frame-timing capture for the scroll script; record the number and the method.
- `xterm-pty-echo`: rendered observation and timing for the echo test.
- `webgl-context-loss-fallback`: forced loss, then input and output check after the switch.
- All runs are on the reference Ubuntu machine inside the Tauri window; record the GPU and session type from the [CW-F01](CW-F01.md) baseline.

## Documentation impact

- [ide-and-git](../ide-and-git.md) (fixture facts once measured), [performance](../performance.md) (measured rows with hardware), [roadmap](../roadmap.md).
