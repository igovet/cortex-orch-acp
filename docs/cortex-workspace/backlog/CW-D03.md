# CW-D03: Desktop B: all required screens and code-first comparison flow

**Status:** archived first-round exploration; not qualified · **Phase:** 1 · **Size:** L

**Rework status:** superseded. Gate D passed on 2026-11-05 for the single revised direction in [CW-D02](CW-D02.md); this mock evidence is historical, not approval.

This is a historical record, not an active implementation direction or Gate D prerequisite. The single revised direction is [CW-D02](CW-D02.md); further implementation follows the backlog and its gates.

## Historical goal
Desktop B: all required screens and code-first comparison flow.

## Requirements
CW-R13, CW-R14; see [requirements](../requirements.md).

## Inputs
[Architecture](../architecture.md), [testing](../testing-and-acceptance.md), and the dependencies below. Validate current source and tool support before implementation.

## Historical deliverables
A clickable Tauri 2 / SolidJS IDE with Mission Control prototype using CW-D01. Cover the shared screens and scenarios in [desktop concepts](../design/desktop-concepts.md), with code-first navigation.

## Historical acceptance
- From the editor open Mission Control and the same task, permission, result/diff and board; a disconnected fixture cannot commit a real action.
- No production backend, credentials or private fixture data; mock outcomes are not runtime evidence.

## Historical dependencies
CW-D01

## Historical validation
Mission Control walkthrough: run the shared scenarios at both desktop viewports and by keyboard; verify editor context remains visible. Record commands, environment, outcomes and unrun cases.

## Documentation impact
Update the relevant planned contract and [roadmap](../roadmap.md) when behavior or qualification changes.

## Status record

- Implemented: clickable IDE with Mission Control at `/b` in `prototypes/cortex-workspace/desktop/src/concepts/b/`. Its 6 concept tests passed on 2026-11-04.
- Walkthrough record: [concept B walkthrough](../design/desktop-concept-b-walkthrough.md). S1, S2 and S3 passed at both viewports. S4 passed in Chromium at 1280×800. S5 passed at 1280×800 (150% and 200%). S6 was not run in the browser.
- Open: the Problems and Output placement with two editor groups needs an owner decision. The editor is a plain text area, not CodeMirror 6. The Git conflict fixture has one file. The native build and an assistive-technology pass were not run. Mock outcomes are not runtime evidence.
