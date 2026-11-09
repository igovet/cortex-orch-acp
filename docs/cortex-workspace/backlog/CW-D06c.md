# CW-D06c: Prototype archive domain tests

**Status:** mock built in the browser prototype; unqualified; Gate D passed 2026-11-05 · **Phase:** 1 · **Size:** M

## Goal
Prototype archive domain tests.

## Requirements
CW-R21, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Test-first archive, configurable retention and purge with an injected clock. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Done ages seven days before archive; default retention is 30 days per machine.
- Negative: Unmerged work requires an explicit deletion answer; refusal prevents archive or purge.

## Dependencies
CW-D06a.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.

## Status record

- Built in `prototypes/cortex-workspace/desktop/src/workspace/domain/archive.ts`: seven-day archive eligibility after Done, the unmerged-work question before discard (refusal keeps the work out of the archive), restore, a 30-day default retention per machine counted from archive time, and a guarded purge that blocks on unexpected changes, live or unknown processes, offline machines, unmerged work and active dependents. The clock is injected from the fixture clock. Tests: `domain/phase4.test.ts`.
- Not confirmed by the build record: the simulated daily purge with a freed-bytes receipt, and the Archive undo receipt (not built).
