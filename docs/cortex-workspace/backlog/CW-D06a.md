# CW-D06a: Prototype task domain and lifecycle tests

**Status:** mock built in the browser prototype; unqualified; Gate D passed 2026-11-05 · **Phase:** 1 · **Size:** L

## Goal
Prototype task domain and lifecycle tests.

## Requirements
CW-R19, CW-R20, CW-R22, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Test-first task, optional worktree, lifecycle and acceptance rules, with pure projections. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Tests cover checkout and worktree, stop, continue and distinct close outcomes.
- Negative: Unknown runtime facts never become success; no UI-owned runtime transitions.

## Dependencies
CW-DM01.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.

## Status record

- Built as in-memory domain code in `prototypes/cortex-workspace/desktop/src/workspace/domain/` (`types.ts`, `core.ts`, `tasks.ts`, `projections.ts`): required description, main-checkout or worktree binding, stop, continue, close and cancel outcomes, evidence acceptance bound to the result digest, and conservative translation of runtime status to board columns. Unknown runtime facts stay unknown and never become success.
- Test-first: the domain tests (`domain/domain.test.ts`) were written before the code, per the build record. Verified 2026-11-05 with the full prototype suite: 331 tests in 14 files passed (see [desktop approval](../design/desktop-approval.md)).
- Not built: any runtime transition. The production contract remains [CW-C01](CW-C01.md) and later items.
