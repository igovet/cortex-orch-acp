# CW-D04: Desktop C: all required screens and focus/attention flow

**Status:** archived first-round exploration; not qualified · **Phase:** 1 · **Size:** L

**Rework status:** superseded. Gate D passed on 2026-11-05 for the single revised direction in [CW-D02](CW-D02.md); this mock evidence is historical, not approval.

This is a historical record, not an active implementation direction or Gate D prerequisite. The single revised direction is [CW-D02](CW-D02.md); further implementation follows the backlog and its gates.

## Historical goal
Desktop C: all required screens and focus/attention flow.

## Requirements
CW-R13, CW-R14; see [requirements](../requirements.md).

## Inputs
[Architecture](../architecture.md), [testing](../testing-and-acceptance.md), and the dependencies below. Validate current source and tool support before implementation.

## Historical deliverables
A clickable Tauri 2 / SolidJS Focus Workspace prototype using CW-D01. Cover the shared screens and scenarios in [desktop concepts](../design/desktop-concepts.md), prioritizing attention and decisions.

## Historical acceptance
- From the attention queue open a decision, reveal task details and reach board, result/diff and code/Git; stale decisions never show real acceptance.
- No production backend, credentials or private fixture data; mock outcomes are not runtime evidence.

## Historical dependencies
CW-D01

## Historical validation
Focus Workspace walkthrough: run the shared scenarios at both desktop viewports and by keyboard; check progressive disclosure. Record commands, environment, outcomes and unrun cases.

## Documentation impact
Update the relevant planned contract and [roadmap](../roadmap.md) when behavior or qualification changes.

## Status record

- Implemented: clickable Focus Workspace at `/c` in `prototypes/cortex-workspace/desktop/src/concepts/c/`. Its 10 concept tests passed on 2026-11-04.
- Walkthrough record: [concept C walkthrough](../design/desktop-concept-c-walkthrough.md). S1 and S2 passed by keyboard at 1280×800. S1 passed at 1920×1080. S2 passed at 1920×1080, with the Abort focus placed by script. State captures exist at both viewports. S5 passed at 1280×800 with the 200% preset; 150% was not run. The desktop → phone → desktop scenario is not covered by this concept.
- Open: focus-visible suppression in the error and loading states (finding C-1 in the [desktop comparison](../design/desktop-comparison.md); `c/c.css:88`), confirmed in source. The status bar shows fixture-wide task and event totals on empty views (finding C-2). The native build and an assistive-technology pass were not run. Mock outcomes are not runtime evidence.
