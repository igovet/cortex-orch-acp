# CW-D05: Revised desktop evaluation and owner Gate D record

**Status:** closed; Gate D passed on 2026-11-05 (owner accepted the clickable prototype) · **Phase:** 1 · **Size:** M

**Build status:** the revised direction ([CW-D02](CW-D02.md), phases P0 to P6) is built as mock-only code. The owner accepted it on 2026-11-05. The P7 checks other than that acceptance (keyboard-only pass, 200% text, reduced motion, per-theme contrast, both viewports) are not recorded as run. Mock evidence is not runtime evidence.

## Goal
Evaluate the single revised direction and record the owner's Gate D decision.

## Requirements
CW-R13, CW-R14; see [requirements](../requirements.md).

## Inputs
[Architecture](../architecture.md), [testing](../testing-and-acceptance.md), and the dependencies below. Validate current source and tool support before implementation.

## Deliverables
A review package for the single revised prototype, including domain and contract test results, rendered captures, keyboard and accessibility observations; record the decision in [desktop approval](../design/desktop-approval.md). An absent or conditional decision does not pass Gate D.

## Acceptance
- The review covers the revised direction; Gate D records owner approval or requested revisions; a request for revisions does not close Gate D.
- No production backend, credentials or private fixture data; mock outcomes are not runtime evidence.

## Dependencies
CW-D02, CW-D07, CW-D08. The design gate passed on 2026-11-05; implementation waits for these dependencies.

## Validation
Inspect the revised-direction screenshots and keyboard, accessibility and performance observations; verify missing evidence is marked unrun. Record commands, environment, outcomes and unrun cases.

## Documentation impact
Update the relevant planned contract and [roadmap](../roadmap.md) when behavior or qualification changes.

## Status record

- The side-by-side comparison of concepts A, B and C is in [desktop comparison](../design/desktop-comparison.md). It uses the same fixture and scenario scripts, records keyboard, accessibility and performance observations, lists the screenshot paths, and names the unrun and unverified checks with their blockers. It does not select a winner.
- [Desktop approval](../design/desktop-approval.md) records the concept build identifiers, fixture revision, screenshot location, environment and observations. Gate D is recorded as passed on 2026-11-05 with the chosen direction, owner, date, prototype identity, required changes and the list of what was not checked.
- The owner accepted the clickable prototype on 2026-11-05, and the decision is recorded in [desktop approval](../design/desktop-approval.md). This item is closed. The record lists what was not checked. Mock outcomes are not runtime evidence.
- 2026-11-05: the owner agreed the static mockups as the design basis and authorised the clickable prototype. Gate D is the decision on that prototype; the owner accepted it, and this item is closed. The round-one comparison above is historical.
