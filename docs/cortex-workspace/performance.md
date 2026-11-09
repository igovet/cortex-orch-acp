# Performance targets

**Status: planned; no implementation or qualification is implied.**

Targets, not measurements or guarantees. Record hardware, OS, GPU, build, dataset and measurement method.

| Metric | Target | Measurement |
|---|---|---|
| Cold desktop board | <700 ms | process launch to interactive board on reference Ubuntu |
| Keystroke to glyph | <50 ms | instrumented input-to-paint, physical devices where feasible |
| Scroll/drag | 16.7 ms per frame, no run over 3 dropped frames | frame timing in representative scenarios |
| Desktop idle memory | <150 MB per window | settled process RSS, report shared-process caveat |
| 10,000 events | 60 fps, viewport-bounded DOM | synthetic history with frame and node counts |
| Terminal | no visible stutter; graphics-context loss falls back safely | bounded PTY output fixture and context-loss test |
| Android Now | <2 s cold start, <5% janky frames | physical-device startup and gfxinfo |

Baseline both 500 tasks and 10,000 events; avoid full-screen rerender on streaming updates. Missing or unreliable instrumentation must be reported, not called a pass. Feasibility and budget failures block a performance claim.

## Proposed budgets pending acceptance

The rows below are proposals from backlog items. They are not measurements and are not accepted targets until the owner accepts them and a recorded measurement exists.

| Metric | Proposed target | Measurement | Source |
|---|---|---|---|
| Git status, diff and log, warm read on a 5,000-commit fixture | 100 ms each, with zero Git subprocess spawns during the call loop | in-process timing on recorded hardware | [CW-G01](backlog/CW-G01.md) |
| Android small-file editor bridge | 256 KiB bound per load or save | payload size and load timing on a physical device | [CW-I05](backlog/CW-I05.md) |

## Editor bundle and input, recorded

These are recorded facts about the built desktop bundle, not a qualification. The input figure comes from WebKitGTK under Xvfb with software rendering, not the reference machine, so it does not satisfy the keystroke target above.

| Fact | Value | Method |
|---|---|---|
| Editor chunk | 335,682 bytes, 106,950 gzipped | `vite build` output, `gzip -9`; loaded the first time a file opens, never at startup |
| Language grammars | one chunk each, largest 103,894 bytes | same build; loaded only for a file of that language |
| All JavaScript in the bundle | 2,059,446 bytes in 20 files | same build, including the 1,144,519-byte main chunk |
| Input to second frame, 5,000-line file | median 33 ms, worst 43 ms over two runs of 40 inserts | `insertText` to two animation frames; two frames at 60 Hz is about 33 ms, so this is bound by the frame rate |
