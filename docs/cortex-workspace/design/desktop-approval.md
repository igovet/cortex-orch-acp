# Desktop approval: Gate D

**Status: Gate D passed on 2026-11-05.** The owner accepted the clickable browser prototype (mock-only) built on the static mockups the owner agreed the same day. This is design acceptance only. Nothing in this record is qualification, a native build or runtime evidence. The round-one concepts A, B and C remain historical comparison material.

The decision record below is the Gate D record. Mobile design may start now that Gate D has passed. Mobile is deferred by the owner's decision of 2026-11-05: mobile implementation still waits for Gate M, and Gate M does not gate desktop implementation. A request for revisions is not approval; a later design change needs a new decision entry here.

## Owner review and decisions (2026-11-04)

Round two was not approved. The text rework direction was reviewed and its owner decisions are recorded in [the proposal](desktop-rework-proposal.ru.md). This was a basis for documentation, not design approval. At that date Gate D was open; it passed on 2026-11-05 (see the decision record).

## Decision record

| Field | Value |
|---|---|
| Gate | D (CW-D05) |
| Gate status | Passed (2026-11-05) |
| Decision | Accepted: the owner accepted the clickable browser prototype |
| Chosen direction | Paseo-style desktop direction based on concept A, built as the clickable prototype in `prototypes/cortex-workspace/desktop/` on the static mockups in `prototypes/cortex-workspace/mockups/` |
| Required changes | Carried into implementation: (1) attachments by drag and drop and by pasting copied screenshots in the message composer and the free-form answer field ([CW-R39](../requirements.md), [CW-U12](../backlog/CW-U12.md), [CW-U14](../backlog/CW-U14.md), [CW-U03](../backlog/CW-U03.md), [CW-U13](../backlog/CW-U13.md)); (2) an automatic sprint moves a checked task to Ready to merge by itself ([CW-R20](../requirements.md), [CW-U05](../backlog/CW-U05.md)); (3) check recovery and acceptance with an unpassed check ([CW-R40](../requirements.md), [CW-U04](../backlog/CW-U04.md), [CW-C07](../backlog/CW-C07.md)) |
| Owner | Project owner (repository owner) |
| Date | 2026-11-05 |

The owner's answer of 2026-11-05 accepted the prototype and added, in English translation: "Accepted. Only when you implement everything: attachments must be movable into the input field, or copied screenshots pasted into it, as they are everywhere." This record treats the acceptance as the design decision and lists the owner's implementation requirements under Required changes. The wording can also be read as acceptance taking effect only once the implementation is verified. If the owner intends that reading, this entry must be re-recorded at that point.

## Gate D record: clickable prototype (2026-11-05)

### Prototype identity

- Prototype: `prototypes/cortex-workspace/desktop/`, a browser build (Vite) with mock-only, in-memory state. Phases P0 to P6 of the [rework proposal](desktop-rework-proposal.ru.md) (section 10) are built. The round-one concepts are no longer routed.
- Source fingerprint, same method as the 2026-11-04 record, run on 2026-11-05: `4012af80117bb7af52841e1697f541f1b377bafd3b10e8200efb912d5a5d6cb3`.
- Build output of `npm run build` on 2026-11-05 (exit 0): `index-C3A3fUx5.js` (27.46 kB), `ui-C0WNkO9j.js` (545.32 kB), `ui-BHRiVpJg.css` (110.80 kB).
- Fixture revision: `cw-fixtures-v3`, seed 13014 (`src/fixtures.ts`). Synthetic data only.
- Design tokens: [accepted static mockup stylesheet](../../../prototypes/cortex-workspace/mockups/styles.css).
- Environment of the recorded checks: Linux, Node v24.14.1, npm 11.11.0, Vite 8.3.5, Vitest 5.1.0. The owner's review environment was not recorded.

### Which state the owner accepted

The owner's answer was recorded at 00:59 UTC on 2026-11-05. The automatic-sprint and check-recovery rules were answered in the same batch and implemented in the prototype later that day. No fingerprint was recorded for the state the owner reviewed, and the prototype directory is not tracked by Git, so that state cannot be recovered. The fingerprint above describes the current tree, which includes the later rule changes. The acceptance covers the design; it is not tied to this fingerprint.

### Checks run for this record

Run from the repository root on 2026-11-05. Mock outcomes are not runtime evidence.

| Command | Exit | Outcome |
|---|---|---|
| `npm --prefix prototypes/cortex-workspace/desktop run typecheck` | 0 | No type errors |
| `npm --prefix prototypes/cortex-workspace/desktop test` | 0 | 15 files, 350 tests passed |
| `npm --prefix prototypes/cortex-workspace/desktop run build` | 0 | Produces the three chunks above; Vite reports the UI chunk above 500 kB as a warning |

### Not checked

- Visual review against the mockups by the agent: not done. The agent takes no screenshots and reads no images, by owner instruction. The owner did the visual review.
- Other CW-D05 phase P7 checks: keyboard-only pass, 200% text, reduced motion, rendered contrast per theme, and both viewports. None is recorded as run. The owner's acceptance is the only P7 outcome recorded.
- Native Tauri 2 build and window: not run. The owner chose the browser only, and Rust is not installed.
- Assistive technology, screen-reader passes and automated accessibility scans: not run.
- Browsers other than headless Chromium (Firefox, Safari/WebKit, WebKitGTK): not run.
- Performance on reference hardware: not measured.
- Runtime behaviour: none. The prototype simulates engines, checks, voice input and message delivery.
- Attachments by drag and drop and paste: not implemented in the prototype, which keeps attached file names only.

### Owner decisions recorded with this gate

- Attachments by drag and drop and paste in the message composer and the free-form answer field: [CW-R39](../requirements.md).
- Automatic sprint: a checked task moves to Ready to merge by itself; the owner confirms there: [CW-R20](../requirements.md).
- Check recovery, and acceptance with an unpassed check only for a technical blocker and with explicit confirmation: [CW-R40](../requirements.md).
- Full wording: [rework proposal](desktop-rework-proposal.ru.md), top section and section 11.

### Post-acceptance request: orchestration settings (2026-11-05)

After the acceptance recorded above, the owner requested a rework of the orchestration settings. The request asks for field-level parity, on each machine, with the Cortex configuration that Paseo exposes, in a more convenient form; for editable MCP servers; and for the tools of a connected MCP server to show their access configuration, which is set in the same place. It is recorded as [CW-R41](../requirements.md) and specified in [SCREENS](../SCREENS.md#orchestration-settings-desktop), with acceptance criteria in [CW-U07](../backlog/CW-U07.md) and [CW-C01](../backlog/CW-C01.md). This note does not approve the orchestration pages, and the current prototype pages do not yet meet the request.

The owner's acceptance remark, recorded under the decision record above, is also recorded as an implementation requirement: attachments must be movable into the message composer and the free-form answer field, or copied screenshots pasted into them, as in common desktop chat applications. It is carried by [CW-R39](../requirements.md) and the backlog items named under Required changes. Whether the acceptance takes effect only once that implementation is verified remains the open reading noted in the decision record; this note does not decide it.

### Native window against the accepted design (2026-11-05)

The first real installed window did not match this acceptance. The OS frame sat above the title band, `.win` was the mockups' inert group, the app's setup notice took the title band's grid row, a colliding `.split` rule stretched the split button's chevron to 360px, and text fell back to whatever `system-ui` resolved to. The window now follows the accepted design: the title band is the only frame and its buttons work, the stylesheet still matches the accepted mockup byte for byte, and the differences are limited to the overrides in `packages/workspace-ui/src/workspace.css` and the bundled faces described in [DESIGN](../DESIGN.md). The owner's review of the installed window is still open; the text-only rendered audit in [platform policy](../platforms.md) does not replace it.

The real app now keeps the accepted task layout too: title band state, plan bar, Inspector, worker pane, queue and the where-it-works choice. Four parts differ from the accepted design, because the machine cannot do them yet, and each says so on the control: a separate worktree per task is off, Stop ends a run instead of pausing it, a message to a worker goes through Cortex, and model, effort, access and language show what the machine sets for every task.

## Recorded evidence (not a decision)

The evidence below was recorded on 2026-11-04 for the owner's comparison. The full comparison is in [desktop comparison](desktop-comparison.md).

### Concept build identifiers

| Concept | Route | Dev server | Build chunks (`npm run build`, exit 0) | Source fingerprint |
|---|---|---|---|---|
| A Task Desk | `/a` | `npm run dev -- --mode a` (127.0.0.1:5181) | `a-DT0HdZ8o.js` (91.95 kB), `a-CxlOHwMR.css` (21.09 kB) | `ae08c1db87dc7fc1b7cf86089a64dde3dde91812e172388db6c7b69ee4299cb2` |
| B IDE with Mission Control | `/b` | `npm run dev -- --mode b` (127.0.0.1:5182) | `b-C-yNO5Zu.js` (115.24 kB), `b-yM29g2E0.css` (21.10 kB) | same |
| C Focus Workspace | `/c` | `npm run dev -- --mode c` (127.0.0.1:5183) | `c-DSE5yPYO.js` (84.26 kB), `c-Bd_gM446.css` (17.06 kB) | same |

The source fingerprint is `find src src-tauri index.html vite.config.ts tsconfig.json package.json package-lock.json -type f | sort | xargs sha256sum | sha256sum`, run in `prototypes/cortex-workspace/desktop/`. All three concepts share one prototype source tree.

### Git state

- Repository HEAD `27e655af26a835a4106c5b9b8aabef84d8f17dcc`, branch `mb/evaluate-standalone-acp-client-proposal`.
- The prototype directory `prototypes/cortex-workspace/desktop/` is untracked. No commit contains the concept builds; the source fingerprint above is the identifier.

### Fixture revision

`cw-fixtures-v1`, seed 13014: 500 tasks, 10,000 events, one running execution. Synthetic data only.

### Screenshots

Local captures at `.design-evidence/cortex-workspace/desktop/a/` (71 entries, including a `_sweep/` folder of superseded captures), `.../b/` (74 files) and `.../c/` (64 files), relative to the repository root. The directory is git-ignored; the captures are not part of the repository. The file index per route, scenario and viewport is in [desktop comparison](desktop-comparison.md), section 5.

### Environment

Linux x86_64; Node v24.14.1; npm 11.11.0; headless Chromium 156 driven by Playwright; viewports 1280×800 and 1920×1080 CSS pixels at device pixel ratio 1. Chromium only. Not run: Firefox, Safari/WebKit, the Tauri 2 native build (Cargo and rustc are not installed) and any physical device.

### Checks recorded on 2026-11-04

| Command | Exit | Outcome |
|---|---|---|
| `npm --prefix prototypes/cortex-workspace/desktop run typecheck` | 0 | No type errors |
| `npm --prefix prototypes/cortex-workspace/desktop test` | 0 | 5 files, 39 tests passed |
| `npm --prefix prototypes/cortex-workspace/desktop run build` | 0 | Shared shell and lazy chunks for A, B and C |

Mock outcomes are not runtime evidence. The checks above verify the prototype builds and its tests pass; they do not verify any product behaviour.

### Accessibility observations

- Contrast of the sampled token pairs passes WCAG 1.4.3 AA by computation. Rendered contrast was not measured.
- Concept A: the custom dialog is not inert to browse-mode navigation (`a/ui.tsx:73`). Confirmed in source; assistive-technology impact not verified.
- Concept C: the focus outline is suppressed in the error and loading states (`c/c.css:88`, `c/index.tsx:269-273`). Confirmed in source; not seen in a rendered engine.
- Concept B: no finding in the files the review read. The review did not cover all of concept B.
- No screen-reader pass and no automated accessibility scan were run for any concept.

### Performance observations

Browser-page timings on development builds in headless Chromium. They are not device, process-launch or reference-hardware measurements and do not qualify the targets in [performance](../performance.md). All three concepts keep long lists bounded (virtualized or windowed) at 500 tasks and 10,000 events. Per-concept values are in [desktop comparison](desktop-comparison.md), section 8.

### Not yet established

Native Tauri 2 build and window; WebKitGTK rendering; screen-reader behaviour; rendered contrast; browser zoom; physical-device and reference-hardware performance; S6 (desktop → phone → desktop) in a browser for concepts B and C; S5 at 1920×1080. Each is listed with its blocker in [desktop comparison](desktop-comparison.md), section 10.

## Owner review note (2026-11-04)

The owner reviewed the round-one concepts (A, B and C) on 2026-11-04 and requested revisions. The owner did not choose a concept and did not approve any of them. **At that date Gate D was open, pending owner decision.** This note was not a decision. Gate D passed on 2026-11-05 (see the Gate D record).

Following the review, one candidate direction was narrowed from concept A in a Paseo-like desktop style, with projects containing tasks, both all-projects and per-project dashboards, and dark and light themes with a toggle. It is specified in [desktop direction](desktop-direction.md), pending owner Gate D. Its contrast figures were recomputed on 2026-11-04; `border.control` is below 3:1 on the hover and selected surfaces (see that document, section 5.2). The round-one fixture revision recorded above (`cw-fixtures-v1`) does not match the v2 fixture mapped by the new direction, which must be aligned before implementation.

## Owner agreement and prototype build (2026-11-05)

The owner agreed the static mockups in `prototypes/cortex-workspace/mockups/` as the design basis and authorised building the clickable prototype. This agreement was not Gate D. Gate D, the owner's decision on the clickable prototype, was recorded as passed on the same date (see the Gate D record above).

Build state of the browser prototype in `prototypes/cortex-workspace/desktop/`, as recorded by its build phases (mock-only, in memory, Chromium only):

- Phase P0 fixtures: revision `cw-fixtures-v3`, seed 13014, with one local and one offline remote machine. The `cw-fixtures-v1` source remains in the tree for the round-one concept code, which is no longer routed.
- Phases P1 to P6 (app frame, task session, review, merge, conflict, Code, Git and worktrees, Home, Inbox, board and sprints, archive and purge, machines and Settings) are built. The terminal is a panel shell only. Gaps are listed in [CW-D02](../backlog/CW-D02.md).
- Phase P7 (keyboard-only pass, 200% text, reduced motion, contrast per theme, both viewports, owner walkthrough): the owner accepted the prototype on the same date. The other P7 checks are not recorded as run (see the Gate D record). It is [CW-D05](../backlog/CW-D05.md), now closed.

Checks observed for this note on 2026-11-05, run from the repository root with `--prefix`: `npm --prefix prototypes/cortex-workspace/desktop run typecheck` exited 0; `npm --prefix prototypes/cortex-workspace/desktop test` exited 0 with 14 files and 331 tests passing; `npm --prefix prototypes/cortex-workspace/desktop run build` exited 0.

Not run: the native Tauri 2 build (the owner chose the browser only, without installing Rust), rendered visual review against the mockups, screen-reader and automated accessibility passes, WebKitGTK and other browsers, and any paid live check (not requested by the owner). Mock outcomes are not runtime evidence.
